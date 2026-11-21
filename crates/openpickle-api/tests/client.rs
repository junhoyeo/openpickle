mod mock_server;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use mock_server::{MockServer, Reply};
use openpickle_api::{
    ClientOptions, Credentials, EndPhoneCallOptions, Error, LoginSession, Method, PickleClient,
    RequestOptions, decode_access_token,
};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

fn jwt(exp_in: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = format!(
        r#"{{"sub":"user-live-test","sid":"sid-test","iat":{now},"exp":{},"sexp":{}}}"#,
        now + exp_in,
        now + 604800
    );
    format!(
        "eyJhbGciOiJFUzI1NiJ9.{}.c2ln",
        URL_SAFE_NO_PAD.encode(claims)
    )
}

fn client(server: &MockServer, credentials: Credentials) -> PickleClient {
    PickleClient::new(ClientOptions {
        credentials,
        base_url: Some(server.url()),
        ..ClientOptions::default()
    })
    .unwrap()
}

const SYNC: &str = r#"{"rooms":[],"rev":4}"#;

#[tokio::test]
async fn login_session_refreshes_once_for_concurrent_requests() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let access = jwt(300);
    let server = MockServer::start({
        let refreshes = refreshes.clone();
        let access = access.clone();
        move |request| {
            if request.path == "/api/login-sessions/refresh" {
                let n = refreshes.fetch_add(1, Ordering::SeqCst) + 1;
                let body = format!(
                    r#"{{"login_session_jwt":"{access}","expires_at":"2026-11-23T00:00:00Z","login_session_token":"ls_next_{n}"}}"#
                );
                // Slow, so the other requests arrive while the refresh is in flight.
                Reply { delay: Duration::from_millis(100), ..Reply::json(200, body) }
            } else {
                Reply::json(200, SYNC)
            }
        }
    })
    .await;

    let rotated = Arc::new(Mutex::new(Vec::new()));
    let session = LoginSession::new("ls_first", {
        let rotated = rotated.clone();
        move |refresh| {
            rotated
                .lock()
                .unwrap()
                .push(refresh.login_session_token.clone());
            Ok(())
        }
    });
    let pickle = client(&server, Credentials::LoginSession(session));

    let calls = (0..8).map(|_| {
        let pickle = pickle.clone();
        tokio::spawn(async move { pickle.sync().await })
    });
    for call in calls {
        assert_eq!(call.await.unwrap().unwrap().rev, 4);
    }
    pickle.sync().await.unwrap();

    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(*rotated.lock().unwrap(), ["ls_next_1"]);
    let requests = server.take();
    let refresh = &requests[0];
    assert_eq!(refresh.path, "/api/login-sessions/refresh");
    assert_eq!(refresh.body, br#"{"login_session_token":"ls_first"}"#);
    assert_eq!(refresh.header("authorization"), None);
    assert_eq!(refresh.header("x-pickle-login-session-rotation"), Some("1"));
    let bearer = format!("Bearer {access}");
    assert_eq!(requests.len(), 10);
    assert!(
        requests[1..]
            .iter()
            .all(|r| r.path == "/api/sync" && r.header("authorization") == Some(&bearer))
    );
}

#[tokio::test]
async fn seeded_access_token_skips_the_first_refresh_and_expired_one_rotates() {
    let server = MockServer::start(|request| {
        if request.path == "/api/login-sessions/refresh" {
            Reply::json(401, r#"{"detail":"session_not_found"}"#)
        } else {
            Reply::json(200, SYNC)
        }
    })
    .await;
    let fresh = LoginSession::new("ls_a", |_| Ok(())).with_access_token(jwt(300));
    client(&server, Credentials::LoginSession(fresh))
        .sync()
        .await
        .unwrap();
    assert_eq!(server.take().len(), 1);

    // Less than a minute left: the client refreshes first, and a failed refresh fails the call.
    let stale = LoginSession::new("ls_b", |_| Ok(())).with_access_token(jwt(30));
    let error = client(&server, Credentials::LoginSession(stale))
        .sync()
        .await
        .unwrap_err();
    let api = error.api().expect("an API error");
    assert_eq!(
        (api.path.as_str(), api.status, api.detail.as_deref()),
        (
            "/api/login-sessions/refresh",
            401,
            Some("session_not_found")
        )
    );
    assert_eq!(server.take().len(), 1, "the sync was not sent");
}

/// A server that answers every refresh with a fresh access token and counts them.
async fn refreshing_server(
    refreshes: Arc<AtomicUsize>,
    sync: impl Fn(&str) -> Reply + Send + Sync + 'static,
) -> MockServer {
    MockServer::start(move |request| {
        if request.path == "/api/login-sessions/refresh" {
            let n = refreshes.fetch_add(1, Ordering::SeqCst) + 1;
            // Each refresh issues a different token: the expiry differs by a second.
            let access = jwt(300 + n as i64);
            Reply::json(
                200,
                format!(
                    r#"{{"login_session_jwt":"{access}","expires_at":"2026-11-23T00:00:00Z","login_session_token":"ls_next_{n}"}}"#
                ),
            )
        } else {
            sync(request.header("authorization").unwrap_or_default())
        }
    })
    .await
}

#[tokio::test]
async fn a_rotation_that_could_not_be_stored_is_stored_before_anything_else() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let server = refreshing_server(refreshes.clone(), |_| Reply::json(200, SYNC)).await;
    let stored = Arc::new(Mutex::new(Vec::new()));
    let disk_full = Arc::new(AtomicUsize::new(1));
    let session = LoginSession::new("ls_first", {
        let (stored, disk_full) = (stored.clone(), disk_full.clone());
        move |refresh| {
            if disk_full.load(Ordering::SeqCst) == 1 {
                return Err(std::io::Error::other("disk full"));
            }
            stored
                .lock()
                .unwrap()
                .push(refresh.login_session_token.clone());
            Ok(())
        }
    });
    let pickle = client(&server, Credentials::LoginSession(session));

    // The server spent `ls_first`, but the new token did not reach the disk: the call fails and
    // the access token that came with it is not used.
    assert!(matches!(pickle.sync().await, Err(Error::Rotate(_))));
    assert!(matches!(pickle.sync().await, Err(Error::Rotate(_))));
    assert!(
        server
            .take()
            .iter()
            .all(|request| request.path == "/api/login-sessions/refresh")
    );

    // Once the disk works, the same refresh is stored. No second token was spent meanwhile.
    disk_full.store(0, Ordering::SeqCst);
    assert_eq!(pickle.sync().await.unwrap().rev, 4);
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(*stored.lock().unwrap(), ["ls_next_1"]);
}

#[tokio::test]
async fn closing_drains_an_in_flight_rotation_without_sending_the_waiting_request() {
    for cancel_request in [false, true] {
        let begun = Arc::new(tokio::sync::Notify::new());
        let server = MockServer::start({
            let begun = begun.clone();
            move |request| {
                if request.path != "/api/login-sessions/refresh" {
                    return Reply::json(200, SYNC);
                }
                begun.notify_one();
                let access = jwt(300);
                let mut reply = Reply::json(
                    200,
                    format!(
                        r#"{{"login_session_jwt":"{access}","expires_at":"2026-11-23T00:00:00Z","login_session_token":"ls_next"}}"#
                    ),
                );
                reply.delay = Duration::from_millis(30);
                reply
            }
        })
        .await;
        let stored = Arc::new(AtomicUsize::new(0));
        let session = LoginSession::new("ls_first", {
            let stored = stored.clone();
            move |_| {
                stored.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let pickle = client(&server, Credentials::LoginSession(session));
        let read = tokio::spawn({
            let pickle = pickle.clone();
            async move { pickle.sync().await }
        });
        tokio::time::timeout(Duration::from_secs(2), begun.notified())
            .await
            .expect("local server did not receive the refresh");
        if cancel_request {
            read.abort();
        }
        pickle.close_login_session().await.unwrap();
        if cancel_request {
            assert!(matches!(read.await, Err(error) if error.is_cancelled()));
        } else {
            assert!(matches!(read.await.unwrap(), Err(Error::SignedOut)));
        }
        assert_eq!(stored.load(Ordering::SeqCst), 1);
        assert!(matches!(pickle.sync().await, Err(Error::SignedOut)));
        assert_eq!(server.take().len(), 1);
    }
}

#[tokio::test]
async fn a_token_the_server_calls_expired_is_refreshed_once_and_the_request_repeated() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    // The seeded token looks valid to this clock, but the server rejects it.
    let seeded = jwt(300);
    let rejected = format!("Bearer {seeded}");
    let server = refreshing_server(refreshes.clone(), move |authorization| {
        if authorization == rejected {
            Reply::json(401, r#"{"detail":"token_expired"}"#)
        } else {
            Reply::json(200, SYNC)
        }
    })
    .await;
    let session = LoginSession::new("ls_first", |_| Ok(())).with_access_token(seeded);
    let pickle = client(&server, Credentials::LoginSession(session));

    assert_eq!(pickle.sync().await.unwrap().rev, 4);
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    let paths: Vec<String> = server
        .take()
        .into_iter()
        .map(|request| request.path)
        .collect();
    assert_eq!(
        paths,
        ["/api/sync", "/api/login-sessions/refresh", "/api/sync"]
    );
}

#[tokio::test]
async fn a_search_sends_the_query_encoded_and_reads_the_matching_runs() {
    let server = MockServer::start(|request| match request.path.as_str() {
        "/api/screens/message-search" => Reply::json(
            200,
            r#"{"kind":"message-search","items":[{"id":"chat:42","kind":"chat","segments":[{"text":"내일 ","match":false},{"text":"비","match":true},{"text":" 온대요","match":false}],"label":"Chat","date_label":"Nov 16","room_id":"","call_id":"","seq":42,"line_id":""},{"id":"call:7","kind":"call","segments":[],"label":"Call","date_label":"Nov 2"}]}"#,
        ),
        _ => Reply::json(404, "{}"),
    })
    .await;
    let pickle = client(&server, Credentials::AccessToken("token".into()));

    let found = pickle.search_messages("비 & 눈").await.unwrap();
    assert_eq!(server.take()[0].query, "query=%EB%B9%84%20%26%20%EB%88%88");
    let first = &found.items[0];
    assert_eq!((first.id.as_str(), first.seq), ("chat:42", 42));
    let matching: Vec<&str> = first
        .segments
        .iter()
        .filter(|segment| segment.is_match)
        .map(|segment| segment.text.as_str())
        .collect();
    assert_eq!(matching, ["비"]);
    // A kind that was never captured may leave out where the result is.
    assert_eq!(
        (found.items[1].seq, found.items[1].room_id.as_str()),
        (0, "")
    );
}

#[tokio::test]
async fn an_offer_is_asked_for_with_its_text_and_its_id() {
    let server = MockServer::start(|request| match request.path.as_str() {
        "/api/screens/explore-sections" => Reply::json(
            200,
            r#"{"kind":"explore-sections","areas":["home"],"popular_ids":["o1"],"sections":[{"section_id":"popular","title":"Popular lately","items":[{"offer_id":"o1","title":"I'll water your plants.","detail":"Every other day.","cta_label":"Water my plants","icon_image":"https://example.com/o1.png","icon_symbol":"house","area":"home","accepted_users_label":"12 people asked for this","steps":["You say which plants","I water them"],"areas":["home"]}],"preview_count":3,"expand_count":0,"areas":["home"]}],"more_items":[],"ranked_items":[{"offer_id":"o1","section_id":"popular","title":"I'll water your plants.","detail":"Every other day.","cta_label":"Water my plants","icon_image":"https://example.com/o1.png","icon_symbol":"house","area":"home","areas":["home"],"accepted_users_label":""}]}"#,
        ),
        "/api/offers/impressions" => Reply::json(200, "{}"),
        "/api/rooms/room/messages" => {
            Reply::json(200, r#"{"status":"ok","responder":"character","seq":null}"#)
        }
        _ => Reply::json(404, "{}"),
    })
    .await;
    let pickle = client(&server, Credentials::AccessToken("token".into()));

    let explore = pickle.get_explore_sections().await.unwrap();
    let offer = &explore.sections[0].items[0];
    assert_eq!(offer.steps.len(), 2);
    // Ranked offers come without their steps and name their section.
    let ranked = &explore.ranked_items[0];
    assert_eq!(
        (ranked.steps.len(), ranked.section_id.as_deref()),
        (0, Some("popular"))
    );

    pickle
        .report_offer_impressions(std::slice::from_ref(&offer.offer_id))
        .await
        .unwrap();
    let options = openpickle_api::SendMessageOptions {
        client_msg_id: Some("m1".into()),
        context: Some(openpickle_api::MessageContext::offer(&offer.offer_id)),
        ..Default::default()
    };
    pickle
        .send_message("room", &offer.cta_label, options)
        .await
        .unwrap();

    let sent: Vec<serde_json::Value> = server.take()[1..]
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    assert_eq!(sent[0], serde_json::json!({ "offer_ids": ["o1"] }));
    assert_eq!(
        sent[1],
        serde_json::json!({
            "text": "Water my plants",
            "client_msg_id": "m1",
            "in_call": false,
            "context": { "kind": "offer", "id": "o1" },
        })
    );
}

#[tokio::test]
async fn handoff_reads_progress_with_open_node_kinds_and_encodes_the_name() {
    let server = MockServer::start(|_| {
        Reply::json(
            200,
            r###"{"kind":"handoff","state":"running","title":"🧭 Plan a day trip","requested_at_epoch_ms":1700000000000.0,"latest_log_text":"Comparing routes","latest_log_kind":"web","handoff_estimate_pct":50,"plan_blocks":[{"type":"prose","markdown":"## Plan\nCompare two routes."}],"log_entries":[],"log_nodes":[{"kind":"future_kind","text":"Checking an option"},{"kind":"report","text":"A useful result"}]}"###,
        )
    })
    .await;
    let pickle = client(&server, Credentials::AccessToken("synthetic-token".into()));

    let handoff = pickle.get_handoff("day trip/서울?").await.unwrap();

    assert_eq!(handoff.state, "running");
    assert_eq!(
        handoff.plan_blocks[0].markdown.as_deref(),
        Some("## Plan\nCompare two routes.")
    );
    assert_eq!(handoff.log_nodes[0].kind, "future_kind");
    assert_eq!(
        handoff.log_nodes[1].text.as_deref(),
        Some("A useful result")
    );
    let request = &server.take()[0];
    assert_eq!(request.path, "/api/screens/handoff");
    assert_eq!(request.query, "name=day%20trip%2F%EC%84%9C%EC%9A%B8%3F");
}

#[tokio::test]
async fn phone_calls_read_live_ending_and_completed_shapes_and_encode_the_id() {
    let line = r#"{"id":"line-1","line_id":"line-1","speaker":"me","text":"Synthetic transcript line","keys":"12#","joins_previous":false,"joins_next":false}"#;
    let server = MockServer::start(move |request| {
        let (state, lifecycle) = match request.path.as_str() {
            "/api/phone-calls/live%2Fid%20%3F" => (
                r#""live":true,"hangup_requested":false,"footer":"Call in progress""#,
                r#""started_at_epoch_ms":1700000000000.0"#,
            ),
            "/api/phone-calls/ending-id" => (
                r#""live":true,"hangup_requested":true,"footer":"Ending call""#,
                r#""started_at_epoch_ms":1700000000000.0,"answered_at_epoch_ms":1700000001000.0"#,
            ),
            "/api/phone-calls/completed-id" => (
                r#""live":false,"hangup_requested":false,"footer":"Call ended""#,
                r#""started_at_epoch_ms":1700000000000.0,"answered_at_epoch_ms":1700000001000.0,"ended_at_epoch_ms":1700000061000.0,"duration_s":60,"outcome":"answered""#,
            ),
            _ => return Reply::json(404, "{}"),
        };
        Reply::json(
            200,
            format!(
                r#"{{"kind":"phone_call","call_id":"synthetic-call","title":"Active call","subtitle_lead":"Speaking with","counterpart_label":"Example contact","subtitle_tail":"","started_label":"Now",{state},"lines":[{line}],{lifecycle}}}"#,
            ),
        )
    })
    .await;
    let pickle = client(&server, Credentials::AccessToken("synthetic-token".into()));

    let live = pickle.get_phone_call("live/id ?").await.unwrap();
    let ending = pickle.get_phone_call("ending-id").await.unwrap();
    let completed = pickle.get_phone_call("completed-id").await.unwrap();

    assert!(live.live && !live.hangup_requested);
    assert!(live.answered_at_epoch_ms.is_none());
    assert_eq!(live.lines[0].keys.as_deref(), Some("12#"));
    assert!(ending.live && ending.hangup_requested);
    assert!(ending.answered_at_epoch_ms.is_some());
    assert!(!completed.live && !completed.hangup_requested);
    assert_eq!(
        (completed.duration_s, completed.outcome.as_deref()),
        (Some(60), Some("answered"))
    );
    assert_eq!(server.take()[0].path, "/api/phone-calls/live%2Fid%20%3F");
}

#[tokio::test]
async fn ending_a_phone_call_preserves_the_retry_action_id_and_false_in_call() {
    let server = MockServer::start(|_| Reply::json(200, r#"{"status":"ok"}"#)).await;
    let pickle = client(&server, Credentials::AccessToken("synthetic-token".into()));
    let options = EndPhoneCallOptions {
        action_id: Some("stable-action".into()),
        in_call: false,
    };

    assert_eq!(
        pickle
            .end_phone_call("synthetic-call", options)
            .await
            .unwrap()
            .status,
        "ok"
    );
    let request = &server.take()[0];
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("POST", "/api/actions")
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
        serde_json::json!({
            "action_id": "stable-action",
            "in_call": false,
            "payload": { "call_id": "synthetic-call" },
            "kind": "end_phone_call",
        })
    );
}

#[tokio::test]
async fn history_pages_go_back_and_thumbnails_are_asked_for_by_encoded_path() {
    let server = MockServer::start(|request| match request.path.as_str() {
        "/api/rooms/room/history/page" => Reply::json(
            200,
            r#"{"v":1,"messages":[{"role":"user","id":"m","text":"사진","at":1.0,"seq":40,"attachments":["attachments/2026-11-05/봄 소풍.jpg"],"image_meta":{"attachments/2026-11-05/봄 소풍.jpg":{"w":1200,"h":900,"blurhash":"LEHV6nWB2yk8"}}}],"reactions":[],"card_states":[],"has_more":true,"next_before":40}"#,
        ),
        _ => Reply {
            content_type: "image/jpeg".into(),
            ..Reply::json(200, "jpeg bytes")
        },
    })
    .await;
    let pickle = client(&server, Credentials::AccessToken("token".into()));

    let page = pickle.get_history_page("room", 100, 250).await.unwrap();
    let path = &page.messages[0].attachments.as_ref().unwrap()[0];
    assert_eq!((page.has_more, page.next_before), (true, 40));
    let thumbnail = pickle.get_attachment_thumbnail(path).await.unwrap();
    assert_eq!(&thumbnail[..], b"jpeg bytes");

    let requests = server.take();
    assert_eq!(requests[0].query, "limit=250&before_seq=100");
    // Each part of the path is encoded on its own; the slashes between them stay.
    assert_eq!(
        requests[1].path,
        "/api/attachments/attachments/2026-11-05/%EB%B4%84%20%EC%86%8C%ED%92%8D.jpg/thumbnail"
    );
    // Pictures are loaded with the token and without the app's identity, as the app does.
    assert_eq!(requests[1].header("authorization"), Some("Bearer token"));
    assert!(requests[0].header("x-client-version").is_some());
    assert!(requests[1].header("x-client-version").is_none());
}

#[tokio::test]
async fn an_attachment_itself_is_asked_for_by_its_encoded_path_without_a_suffix() {
    let server = MockServer::start(|_| Reply {
        content_type: "application/pdf".into(),
        ..Reply::json(200, "%PDF-1.7 made-up bytes")
    })
    .await;
    let pickle = client(&server, Credentials::AccessToken("token".into()));

    let file = pickle
        .get_attachment("attachments/2026-11-05/여행 계획 v2.pdf")
        .await
        .unwrap();

    assert_eq!(&file[..], b"%PDF-1.7 made-up bytes");
    let request = &server.take()[0];
    assert_eq!(
        request.path,
        "/api/attachments/attachments/2026-11-05/%EC%97%AC%ED%96%89%20%EA%B3%84%ED%9A%8D%20v2.pdf"
    );
    // Like its thumbnail, the file is loaded with the token alone.
    assert_eq!(request.header("authorization"), Some("Bearer token"));
    assert!(request.header("x-client-version").is_none());
}

#[tokio::test]
async fn api_errors_carry_status_detail_and_body() {
    let server = MockServer::start(|request| match request.path.as_str() {
        "/api/device/location/token" => Reply::json(409, r#"{"detail":"location_disabled"}"#),
        _ => Reply {
            content_type: "text/plain".into(),
            ..Reply::json(502, "bad gateway")
        },
    })
    .await;
    let pickle = client(&server, Credentials::AccessToken("token".into()));

    let error = pickle.request_location_token().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "POST /api/device/location/token failed with 409: location_disabled"
    );
    let api = error.api().unwrap();
    assert_eq!(api.method, "POST");
    assert_eq!(api.detail.as_deref(), Some("location_disabled"));
    assert_eq!(&api.body[..], br#"{"detail":"location_disabled"}"#);

    let error = pickle.sync().await.unwrap_err();
    let api = error.api().unwrap();
    assert_eq!(
        (api.status, api.detail.as_deref(), &api.body[..]),
        (502, None, &b"bad gateway"[..])
    );
}

#[tokio::test]
async fn signed_out_client_sends_no_bearer_request() {
    let server = MockServer::start(|_| Reply::json(200, "{}")).await;
    let pickle = client(&server, Credentials::SignedOut);
    assert!(matches!(pickle.sync().await, Err(Error::SignedOut)));
    assert!(server.take().is_empty());
}

#[tokio::test]
async fn request_options_merge_into_the_generated_request() {
    let server = MockServer::start(|_| Reply::json(200, "{}")).await;
    let pickle = client(&server, Credentials::AccessToken("generated".into()));

    // Query options join a path that already has a query; commas stay unescaped.
    let options = RequestOptions {
        query: vec![("scopes".into(), "a,b".into())],
        ..Default::default()
    };
    pickle
        .request::<serde_json::Value>(Method::GET, "/api/profile?region=KR", options)
        .await
        .unwrap();

    // A header override replaces the generated header instead of joining it.
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer override"));
    let options = RequestOptions {
        headers,
        ..Default::default()
    };
    pickle
        .request::<serde_json::Value>(Method::GET, "/api/sync", options)
        .await
        .unwrap();

    let requests = server.take();
    assert_eq!(
        (requests[0].path.as_str(), requests[0].query.as_str()),
        ("/api/profile", "region=KR&scopes=a,b")
    );
    let authorization: Vec<&str> = requests[1]
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(authorization, ["Bearer override"]);
}

#[test]
fn token_claims_are_read_as_utf8() {
    let claims = r#"{"sub":"피클","sid":"s","iat":1,"exp":2,"sexp":3}"#;
    let token = format!("Bearer h.{}.s", URL_SAFE_NO_PAD.encode(claims));
    assert_eq!(decode_access_token(&token).unwrap().sub, "피클");
}

#[tokio::test]
async fn kakao_and_sms_issue_the_same_session_without_bearer_or_app_proof() {
    let server = MockServer::start(|_| Reply::json(200,
        r#"{"status":"ok","user_id":"fixture-user","login_session_token":"fixture-session","login_session_jwt":"fixture-jwt","expires_at":"2030-01-01T00:00:00Z"}"#
    )).await;
    let pickle = client(&server, Credentials::default());
    let kakao = pickle
        .login_with_kakao_id_token("fixture-id")
        .await
        .unwrap();
    let sms = pickle.verify_phone_code("pending", "123456").await.unwrap();
    assert_eq!(kakao.login_session_token, sms.login_session_token);
    let calls = server.take();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&calls[0].body).unwrap(),
        serde_json::json!({"provider":"kakao","kind":"id_token","id_token":"fixture-id"})
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&calls[1].body).unwrap(),
        serde_json::json!({"pending_id":"pending","code":"123456","kind":"phone_code"})
    );
    for call in calls {
        assert_eq!(call.path, "/api/login-sessions");
        assert_eq!(call.header("authorization"), None);
        assert_eq!(call.header("x-pickle-sign-in-flow"), Some("2"));
    }
}
