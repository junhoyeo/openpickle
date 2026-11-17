//! Replays a Proxyman capture (`.proxymanlogv2`) through `PickleClient` against a local mock
//! server and checks that, for every recorded request, the client sends the same method, path,
//! raw query string, JSON body and app headers, and that it parses the recorded response (or
//! fails with the recorded status). Nothing goes to the network. Output never contains bodies,
//! header values or ids, because captures hold tokens and private messages.
//!
//!   cargo run -p openpickle-api --example verify_capture -- <capture.proxymanlogv2>

#[path = "../tests/mock_server/mod.rs"]
mod mock_server;

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures_util::StreamExt as _;
use mock_server::{MockServer, Recorded, Reply};
use openpickle_api::{
    ClientOptions, ClientProfile, Credentials, EndPhoneCallOptions, Error, LogStoreListingsRequest,
    MessageContext, PermissionSnapshot, PickleClient, PickleEvent, SendMessageOptions,
    SessionRequest,
};
use serde_json::Value;

/// Headers the client leaves to the HTTP stack, and so does not compare:
/// - `host`, `content-length`: written by hyper from the URL and the body.
/// - `accept-encoding`: written by reqwest's decompression layer (`gzip, br`; the app's
///   CFNetwork sends `gzip, deflate, br`).
/// - `connection`: hop-by-hop; CFNetwork sends `keep-alive`, hyper relies on the HTTP/1.1
///   default and HTTP/2 forbids it.
const TRANSPORT_HEADERS: [&str; 4] = ["host", "connection", "content-length", "accept-encoding"];

/// Endpoints the client leaves out on purpose (see `ts/README.md`).
const NOT_COVERED: [&str; 3] = [
    "/api/telemetry",
    "/api/push-token",
    "/api/secure/log/verified",
];

const KNOWN_EVENTS: [&str; 6] = ["chat", "usage", "status", "widget", "sync", "room"];

struct Flow {
    name: String,
    method: String,
    path: String,
    query: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    status: u16,
    response_type: String,
    response_body: Vec<u8>,
}

fn read_capture(file: &str) -> Result<Vec<Flow>, Box<dyn std::error::Error>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file)?)?;
    let mut flows = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name()?.into_owned();
        let mut raw = Vec::new();
        entry.read_to_end(&mut raw)?;
        let entry: Value = serde_json::from_slice(&raw)?;
        let request = &entry["request"];
        let method = request["method"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let Some(status) = entry["response"]["status"]["code"].as_u64() else {
            continue;
        };
        if method == "CONNECT" {
            continue;
        }
        let full_path = request["fullPath"]
            .as_str()
            .ok_or("request without fullPath")?;
        let after_scheme = full_path
            .split_once("://")
            .map_or(full_path, |(_, rest)| rest);
        let target = after_scheme
            .find('/')
            .map_or("/", |slash| &after_scheme[slash..]);
        let target = target.split_once('#').map_or(target, |(target, _)| target);
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let response_headers = header_map(&entry["response"]["header"]);
        flows.push(Flow {
            name,
            method,
            path: path.to_owned(),
            query: query.to_owned(),
            headers: header_map(&request["header"]),
            body: base64_field(&request["bodyData"])?,
            status: u16::try_from(status)?,
            response_type: response_headers
                .get("content-type")
                .cloned()
                .unwrap_or_else(|| "application/json".into()),
            response_body: base64_field(&entry["response"]["bodyData"])?,
        });
    }
    Ok(flows)
}

fn header_map(header: &Value) -> BTreeMap<String, String> {
    let entries = header["entries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    entries
        .iter()
        .filter_map(|e| {
            Some((
                e["key"]["name"].as_str()?.to_ascii_lowercase(),
                e["value"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn base64_field(value: &Value) -> Result<Vec<u8>, base64::DecodeError> {
    value
        .as_str()
        .map_or(Ok(Vec::new()), |data| STANDARD.decode(data))
}

/// `decodeURIComponent` for path segments.
fn decode_segment(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The endpoint label: path with private resource ids replaced.
fn label(flow: &Flow) -> String {
    if flow.path.starts_with("/api/attachments/") && flow.path.ends_with("/thumbnail") {
        return format!("{} /api/attachments/{{path}}/thumbnail", flow.method);
    }
    let segments: Vec<&str> = flow.path.split('/').collect();
    let mut out = Vec::with_capacity(segments.len());
    for (i, segment) in segments.iter().enumerate() {
        let previous = i.checked_sub(1).map(|p| segments[p]);
        out.push(match previous {
            Some("rooms") => "{room}",
            Some("phone-calls") => "{call_id}",
            Some("nodes") if segment.len() == 43 => "{node}",
            _ => segment,
        });
    }
    format!("{} {}", flow.method, out.join("/"))
}

/// A client that identifies itself exactly as the recorded app launch did.
fn client_for(flow: &Flow, json: &Value, http: &reqwest::Client, base_url: &str) -> PickleClient {
    let h = &flow.headers;
    let get = |name: &str| h.get(name).cloned();
    let mut profile = ClientProfile::ios();
    if let Some(v) = get("user-agent") {
        profile.user_agent = v;
    }
    if let Some(v) = get("accept-language") {
        profile.accept_language = v;
    }
    if let Some(v) =
        get("x-pickle-vocabulary").or_else(|| json["vocabulary"].as_str().map(str::to_owned))
    {
        profile.vocabulary = v;
    }
    if h.contains_key("x-client-version") {
        profile.app_version = get("x-client-version").unwrap_or_default();
        profile.app_build = get("x-client-build").unwrap_or_default();
        profile.language = get("x-pickle-language").unwrap_or(profile.language);
        profile.language_source =
            get("x-pickle-language-source").unwrap_or(profile.language_source);
        profile.device_region = get("x-pickle-device-region").unwrap_or(profile.device_region);
        profile.storefront_country = get("x-pickle-storefront-country");
    }
    if let Some(v) = get("x-client-timezone") {
        profile.timezone = v;
    }
    if let Some(v) = get("x-client-platform") {
        profile.platform = v;
    }
    let credentials = match h
        .get("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        Some(token) => Credentials::AccessToken(token.to_owned()),
        None => Credentials::SignedOut,
    };
    PickleClient::new(ClientOptions {
        credentials,
        device_id: json["device_id"].as_str().map(str::to_owned),
        launch_id: get("x-client-launch-id"),
        profile,
        base_url: Some(base_url.to_owned()),
        http: Some(http.clone()),
    })
    .expect("client options are valid")
}

/// Calls the client method for the flow's endpoint. `None` when the client has none.
async fn call(client: &PickleClient, flow: &Flow, json: &Value) -> Option<Result<(), String>> {
    fn done<T>(result: Result<T, Error>) -> Result<(), Error> {
        result.map(drop)
    }
    fn from<T: serde::de::DeserializeOwned>(json: &Value) -> T {
        serde_json::from_value(json.clone()).expect("recorded request body has the expected shape")
    }
    let s = |key: &str| json[key].as_str().unwrap_or_default().to_owned();
    let query: Vec<(String, String)> = flow
        .query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode_segment(k), decode_segment(v))
        })
        .collect();
    let q = |key: &str| {
        query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let segments: Vec<String> = flow.path.split('/').map(decode_segment).collect();
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();

    let result = match (flow.method.as_str(), &segments[..]) {
        ("POST", ["", "api", "login-sessions", "phone-code"]) => {
            let region = json["region"].as_str();
            done(
                client
                    .request_phone_code(&s("phone"), region, json["carrier"].as_str())
                    .await,
            )
        }
        ("POST", ["", "api", "login-sessions"]) => {
            done(client.verify_phone_code(&s("pending_id"), &s("code")).await)
        }
        ("POST", ["", "api", "login-sessions", "refresh"]) => done(
            client
                .refresh_login_session(&s("login_session_token"))
                .await,
        ),
        ("POST", ["", "api", "account", "register"]) => done(client.register_account().await),
        ("POST", ["", "api", "session"]) => done(
            client
                .start_session_with(&from::<SessionRequest>(json))
                .await,
        ),
        ("GET", ["", "api", "sync"]) => done(client.sync().await),
        ("GET", ["", "api", "events"]) => return Some(drain_events(client).await),
        ("GET", ["", "api", "rooms", room, "history"]) => done(client.get_history(room).await),
        ("GET", ["", "api", "rooms", room, "history", "page"]) => done(
            client
                .get_history_page(
                    room,
                    q("before_seq").parse().unwrap_or_default(),
                    q("limit").parse().unwrap_or_default(),
                )
                .await,
        ),
        ("GET", ["", "api", "attachments", rest @ ..]) if rest.last() == Some(&"thumbnail") => {
            // The segments are decoded already; the client encodes them again.
            let path = rest[..rest.len() - 1].join("/");
            done(client.get_attachment_thumbnail(&path).await)
        }
        ("GET", ["", "api", "rooms", room, "history", "delta"]) => {
            let limit = q("limit").parse().ok();
            done(
                client
                    .get_history_delta(room, q("after_seq").parse().unwrap_or_default(), limit)
                    .await,
            )
        }
        ("POST", ["", "api", "rooms", room, "messages"]) => {
            let options = SendMessageOptions {
                client_msg_id: Some(s("client_msg_id")),
                in_call: json["in_call"].as_bool().unwrap_or_default(),
                context: json.get("context").map(from::<MessageContext>),
            };
            done(client.send_message(room, &s("text"), options).await)
        }
        ("PUT", ["", "api", "rooms", room, "read"]) => done(
            client
                .mark_read(room, json["seq"].as_i64().unwrap_or_default())
                .await,
        ),
        ("GET", ["", "api", "phone-calls", call_id]) => done(client.get_phone_call(call_id).await),
        ("POST", ["", "api", "actions"]) if json["kind"] == "end_phone_call" => {
            let options = EndPhoneCallOptions {
                action_id: json["action_id"].as_str().map(str::to_owned),
                in_call: json["in_call"].as_bool().unwrap_or_default(),
            };
            done(
                client
                    .end_phone_call(
                        json["payload"]["call_id"].as_str().unwrap_or_default(),
                        options,
                    )
                    .await,
            )
        }
        ("GET", ["", "api", "character"]) => done(client.get_character().await),
        ("GET", ["", "api", "profile"]) => done(client.get_profile(Some(&q("region"))).await),
        ("GET", ["", "api", "social", "snapshot"]) => {
            let scopes = q("scopes");
            done(
                client
                    .get_social_snapshot(&scopes.split(',').collect::<Vec<_>>())
                    .await,
            )
        }
        ("GET", ["", "api", "screens", "explore-sections"]) => {
            done(client.get_explore_sections().await)
        }
        ("POST", ["", "api", "offers", "impressions"]) => done(
            client
                .report_offer_impressions(&from::<Vec<String>>(&json["offer_ids"]))
                .await,
        ),
        ("GET", ["", "api", "screens", "message-search"]) => {
            done(client.search_messages(&q("query")).await)
        }
        ("GET", ["", "api", "screens", kind]) if !query.is_empty() => {
            let pairs: Vec<(&str, &str)> = query
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            done(client.get_screen(kind, &pairs).await)
        }
        ("GET", ["", "api", "screens", "handoffs"]) => done(client.get_handoffs().await),
        ("GET", ["", "api", "screens", "extra-conversations"]) => {
            done(client.get_extra_conversations().await)
        }
        ("GET", ["", "api", "screens", "roulette-promo"]) => {
            done(client.get_roulette_promo().await)
        }
        ("GET", ["", "api", "screens", kind]) => done(client.get_screen(kind, &[]).await),
        ("PUT", ["", "api", "device", "permissions"]) => done(
            client
                .report_permissions(&from::<Vec<PermissionSnapshot>>(&json["snapshots"]))
                .await,
        ),
        ("POST", ["", "api", "device", "location", "token"]) => {
            done(client.request_location_token().await)
        }
        ("GET", ["", "api", "ui-definitions"]) => done(client.get_ui_definitions().await),
        ("GET", ["", "api", "secure", "account"]) => done(client.get_secure_account().await),
        ("GET", ["", "api", "secure", "nodes"]) => {
            done(client.get_enclave_nodes(Some(&q("nonce"))).await)
        }
        ("POST", ["", "api", "secure", "nodes", node, "status"]) => {
            done(client.get_node_status(node, json["nonce"].as_str()).await)
        }
        ("GET", ["", "api", "secure", "log", "nodes"]) => done(client.get_log_nodes().await),
        ("POST", ["", "api", "secure", "log", "store", "listings"]) => done(
            client
                .get_log_store_listings(&from::<LogStoreListingsRequest>(json))
                .await,
        ),
        ("GET", ["", "api", "secure", "ledger"]) => done(client.get_shared_ledger().await),
        ("GET", ["", "api", "secure", "transfers"]) => done(client.get_transfers().await),
        _ => return None,
    };
    Some(match result {
        Ok(()) if flow.status >= 300 => Err(format!("status {} did not fail", flow.status)),
        Ok(()) => Ok(()),
        Err(Error::Api(api)) if api.status == flow.status => Ok(()),
        Err(error) => Err(describe(&error)),
    })
}

/// Drains the event stream; a known event that does not fit its type counts as a failure.
async fn drain_events(client: &PickleClient) -> Result<(), String> {
    let stream = client.events().await.map_err(|e| describe(&e))?;
    let mut stream = std::pin::pin!(stream);
    while let Some(event) = stream.next().await {
        match event.map_err(|e| describe(&e))? {
            PickleEvent::Unknown { name, data } if KNOWN_EVENTS.contains(&name.as_str()) => {
                let kind = data["kind"].as_str().unwrap_or("-").to_owned();
                return Err(format!("`{name}` event of kind `{kind}` did not parse"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// An error without bodies, ids or values.
fn describe(error: &Error) -> String {
    match error {
        Error::Api(api) => format!("API error with status {}", api.status),
        Error::Decode { source, .. } => format!(
            "response did not parse ({:?} error at line {} column {})",
            source.classify(),
            source.line(),
            source.column()
        ),
        Error::Http(e) => format!(
            "HTTP error (timeout: {}, connect: {})",
            e.is_timeout(),
            e.is_connect()
        ),
        other => format!("{other}"),
    }
}

fn compare(flow: &Flow, sent: &[Recorded]) -> Vec<String> {
    let Some(sent) = sent.first() else {
        return vec!["no request was sent".into()];
    };
    let mut problems = Vec::new();
    if sent.method != flow.method {
        problems.push(format!("method {} != {}", sent.method, flow.method));
    }
    if sent.path != flow.path {
        problems.push("path differs".into());
    }
    if sent.query != flow.query {
        problems.push("query string differs".into());
    }
    let parse =
        |body: &[u8]| (!body.is_empty()).then(|| serde_json::from_slice::<Value>(body).ok());
    let (ours, theirs) = (parse(&sent.body), parse(&flow.body));
    if ours != theirs {
        let keys = |v: &Option<Option<Value>>| match v {
            Some(Some(Value::Object(map))) => map.keys().cloned().collect::<Vec<_>>().join(","),
            Some(_) => "non-object".into(),
            None => "none".into(),
        };
        problems.push(format!(
            "JSON body differs (keys sent: {}; recorded: {})",
            keys(&ours),
            keys(&theirs)
        ));
    }
    let sent_headers: BTreeMap<&str, &str> = sent
        .headers
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    for (name, value) in &flow.headers {
        if TRANSPORT_HEADERS.contains(&name.as_str()) {
            continue;
        }
        match sent_headers.get(name.as_str()) {
            None => problems.push(format!("missing header {name}")),
            Some(ours) if ours != value => problems.push(format!("header {name} differs")),
            Some(_) => {}
        }
    }
    for name in sent_headers.keys() {
        if !TRANSPORT_HEADERS.contains(name) && !flow.headers.contains_key(*name) {
            problems.push(format!("extra header {name}"));
        }
    }
    problems
}

#[tokio::main]
async fn main() -> ExitCode {
    let Some(file) = std::env::args().nth(1) else {
        eprintln!("usage: verify_capture <capture.proxymanlogv2>");
        return ExitCode::from(2);
    };
    let mut flows = match read_capture(&file) {
        Ok(flows) => flows,
        Err(error) => {
            eprintln!("cannot read {file}: {error}");
            return ExitCode::from(2);
        }
    };
    let number = |name: &str| {
        name.trim_start_matches(|c: char| !c.is_ascii_digit())
            .parse::<u64>()
            .unwrap_or(0)
    };
    flows.sort_by_key(|flow| number(&flow.name));

    // The flow being replayed; the server answers every request with its recorded response.
    let current: Arc<Mutex<Option<Reply>>> = Arc::new(Mutex::new(None));
    let server = MockServer::start({
        let current = current.clone();
        move |_| {
            current
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| Reply::json(500, "{}"))
        }
    })
    .await;
    let base_url = server.url();
    let http = reqwest::Client::builder()
        .gzip(true)
        .brotli(true)
        .build()
        .expect("HTTP client");

    let mut results: Vec<(String, usize, Vec<String>)> = Vec::new();
    let mut uncovered: Vec<(String, usize)> = Vec::new();
    for flow in &flows {
        let label = label(flow);
        let json: Value = serde_json::from_slice(&flow.body).unwrap_or(Value::Null);
        *current.lock().unwrap() = Some(Reply {
            status: flow.status,
            content_type: flow.response_type.clone(),
            body: flow.response_body.clone(),
            delay: std::time::Duration::ZERO,
        });
        let client = client_for(flow, &json, &http, &base_url);
        let outcome = if NOT_COVERED.contains(&flow.path.as_str()) {
            None
        } else {
            call(&client, flow, &json).await
        };
        let sent = server.take();
        let Some(outcome) = outcome else {
            match uncovered.iter_mut().find(|(l, _)| *l == label) {
                Some((_, count)) => *count += 1,
                None => uncovered.push((label, 1)),
            }
            continue;
        };
        let mut problems = compare(flow, &sent);
        if let Err(problem) = outcome {
            problems.push(problem);
        }
        if sent.len() > 1 {
            problems.push(format!("sent {} requests", sent.len()));
        }
        let index = match results.iter().position(|(l, ..)| *l == label) {
            Some(index) => index,
            None => {
                results.push((label, 0, Vec::new()));
                results.len() - 1
            }
        };
        let (_, count, all) = &mut results[index];
        *count += 1;
        all.extend(problems);
    }

    let mut failed = 0;
    for (label, count, problems) in &results {
        let mut seen = HashSet::new();
        let problems: Vec<&String> = problems.iter().filter(|p| seen.insert(*p)).collect();
        if !problems.is_empty() {
            failed += 1;
        }
        println!(
            "{} {label} x{count}",
            if problems.is_empty() { "ok  " } else { "FAIL" }
        );
        for problem in problems {
            println!("       {problem}");
        }
    }
    for (label, count) in &uncovered {
        println!("skip {label} x{count} (no client method)");
    }
    println!(
        "\n{}/{} endpoints match, {} not covered",
        results.len() - failed,
        results.len(),
        uncovered.len()
    );
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
