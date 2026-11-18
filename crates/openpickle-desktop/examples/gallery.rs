//! `phone-demo float` shows a synthetic live transcript and confirmation controls. It has no
//! backend or action subscriber and cannot place or end a real call. `ended` shows the call
//! after it ended, `call=<file>` the transcript in that JSON file, and `app phone` the card of
//! the call in the chat. `app float tasks` shows a made-up running task card; add `task-open`
//! for its progress sheet, `task-expanded` for the tall scrolled state, or `task-plan` for its
//! plan. `task=<file>` loads a `GET /api/screens/handoff` JSON body for private comparisons.
//!
//! Opens the chat screen with made-up messages that have the same structure as the Figma frames,
//! to compare the two side by side: `cargo run -p openpickle-desktop --example gallery`.
//! Pass `login` to see the sign-in screen instead, or `app` to run the whole window on these
//! messages: the menu, and a search that looks through them (Cmd-F).
//!
//! After `app`, further arguments are played into the window as keys, so a flow can be looked
//! at without the real keyboard: `key:<keystroke>`, `type:<text>` and `wait:<ms>`. `float`
//! keeps the window above the others and leaves the keyboard focus where it is. For example:
//!
//!     gallery app float key:cmd-f type:취소표 wait:600 key:down key:enter

use std::sync::Arc;

use chrono::Local;
use gpui::{
    AnyWindowHandle, App, AppContext, Application, AsyncApp, Bounds, Focusable, Keystroke,
    Modifiers, Window, WindowBounds, WindowKind, WindowOptions, point, px, size,
};
use openpickle_desktop::{
    app::Shell,
    assets::Assets,
    content::{Block, parse_message},
    model::{
        Attachment, AttachmentKind, Explore, Invite, Message, Offer, OfferSection, Part, Person,
        Request, RequestStatus, SearchResult, Side, Social, build_rows,
    },
    protocol::{Command, HandoffCard, Update},
    ui::{self, chat::ChatView, login::LoginView},
};

fn person(name: &str, note: &str) -> Person {
    Person {
        id: name.into(),
        name: name.into(),
        picture: String::new(),
        note: note.into(),
    }
}

/// Fake phone data used only by the offline native gallery: a made-up call to a library's
/// answering system. `call=<file>` shows the transcript in that JSON file instead.
fn sample_phone_call(call_id: &str, live: bool) -> openpickle_api::PhoneCallTranscript {
    if let Some(path) = std::env::args().find_map(|arg| arg.strip_prefix("call=").map(String::from))
    {
        let mut call: openpickle_api::PhoneCallTranscript =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        call.call_id = call_id.into();
        return call;
    }
    let line = |id: u32, speaker: &str, text: &str, keys: Option<&str>, joins: (bool, bool)| {
        serde_json::json!({
            "id": format!("l{id}"), "line_id": id.to_string(), "speaker": speaker, "text": text,
            "keys": keys, "joins_previous": joins.0, "joins_next": joins.1,
        })
    };
    let now = chrono::Utc::now().timestamp_millis() as f64;
    let mut call = serde_json::json!({
        "kind": "phone_call", "call_id": call_id, "title": "도서관 자동 응답 안내",
        "subtitle_lead": "Pickle\u{2019}s call with ", "counterpart_label": "02-000-0000",
        "subtitle_tail": "", "started_label": "Today 1:36 PM", "live": live,
        "hangup_requested": false,
        "footer": if live { "Pickle is on the call" } else { "Call ended · Today 1:38 PM" },
        "started_at_epoch_ms": now - 100_000., "answered_at_epoch_ms": now - 96_000.,
        "lines": [
            line(1, "me", "여보세요?", None, (false, false)),
            line(2, "them", "안녕하세요, 샘플 시립 도서관입니다.", None, (false, true)),
            line(3, "them", "운영 시간 안내는 1번, 대출 연장은 2번, 직원 연결은 0번.", None, (true, true)),
            line(4, "them", "열람실 좌석 안내는 3번, 행사 신청은 4번을 눌러 주십시오. For English, press 6.", None, (true, true)),
            line(5, "them", "다시 듣기는 9번을 눌러 주십시오.", None, (true, false)),
            line(6, "me", "1", Some("1"), (false, false)),
            line(7, "them", "평일은 오전 9시부터 오후 10시까지, 주말은 오후 6시까지 운영합니다.", None, (false, false)),
            line(8, "me", "감사합니다. 필요한 내용을 모두 확인했어요. 좋은 하루 보내세요.", None, (false, false)),
            line(9, "them", "이용해 주셔서 감사합니다. 다른 안내가 필요하시면 별표를 눌러 주십시오.", None, (false, false)),
        ],
    });
    if !live {
        call["ended_at_epoch_ms"] = (now - 500.).into();
        call["duration_s"] = 95.into();
    }
    serde_json::from_value(call).unwrap()
}

/// The progress of a made-up task, or with `task=<file>` the one in that JSON file, which is
/// the body of a `GET /api/screens/handoff`.
fn sample_task_screen(running: bool) -> openpickle_api::HandoffScreen {
    if let Some(path) = std::env::args().find_map(|arg| arg.strip_prefix("task=").map(String::from))
    {
        return serde_json::from_slice(&std::fs::read(path).expect("read task JSON"))
            .expect("parse GET /api/screens/handoff body");
    }
    let node = |kind: &str, text: &str| openpickle_api::HandoffLogNode {
        kind: kind.into(),
        text: Some(text.into()),
    };
    openpickle_api::HandoffScreen {
        kind: "handoff".into(),
        state: if running { "running" } else { "done" }.into(),
        title: "🧭 Plan a museum afternoon".into(),
        requested_at_epoch_ms: chrono::Utc::now().timestamp_millis() as f64 - 137_000.,
        latest_log_text: Some("Comparing walking times between the two museums".into()),
        latest_log_kind: Some("shell".into()),
        handoff_estimate_pct: Some(50.),
        plan_blocks: vec![openpickle_api::HandoffPlanBlock {
            r#type: "prose".into(),
            markdown: Some(
                "## Afternoon plan\n\nCompare opening hours, travel time, and one nearby café."
                    .into(),
            ),
        }],
        log_entries: Vec::new(),
        log_nodes: vec![
            node("report", "The request was handed to the trip researcher."),
            node("start", "Work started"),
            node(
                "page",
                "Reading notes/trips/museum-afternoon/visitor-guide.md",
            ),
            node("web", "Searching \"museum late opening hours Thursday\""),
            node("browser", "Opening https://example.com/transit/planner"),
            node(
                "report",
                "Both museums are open late. Next I compare the routes.",
            ),
            node("shell", "Comparing walking times between the two museums"),
        ],
    }
}

/// What the handoff list says about the same task.
fn sample_task_item(running: bool) -> openpickle_api::HandoffItem {
    let task = sample_task_screen(running);
    openpickle_api::HandoffItem {
        name: "sample-task".into(),
        title: task.title,
        subtitle: (task.state != "running").then(|| "Nov 18, 2:10pm".into()),
        state: task.state,
        requested_at_epoch_ms: task.requested_at_epoch_ms,
        finished_at_epoch_ms: None,
        result_text: None,
        time_text: "Now".into(),
        latest_log_text: task.latest_log_text,
        latest_log_kind: task.latest_log_kind,
        phone_call_id: None,
        phone_status_text: None,
        phone_answered_at_epoch_ms: None,
    }
}

/// Made-up profile values, never copied from an account or a capture.
fn sample_profile() -> openpickle_desktop::profile::ProfileData {
    use openpickle_desktop::profile::{
        ConnectionPreview, ConnectionSummary, ProfileData, TicketCounts, UsageSnapshot,
    };
    ProfileData {
        display_name: "Sample account".into(),
        usage: Some(UsageSnapshot {
            allowance: 100,
            remaining: 70,
            reset_at: Some((chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339()),
            period: Some("day".into()),
            ticket_auto_use: false,
            ticket_order: Some("half_first".into()),
            tickets: TicketCounts {
                full: Some(2),
                half: Some(1),
            },
        }),
        connections: Some(ConnectionSummary {
            enabled_count: Some(1),
            enabled: vec![ConnectionPreview {
                source: "calendar".into(),
                label: "Calendar".into(),
                initial: Some("C".into()),
            }],
        }),
        ..ProfileData::default()
    }
}

/// Made-up friends and a made-up invite link.
fn sample_social() -> Social {
    let names = [
        "민지", "Jun", "서연", "Haru", "도윤", "Mina", "지호", "Sora", "예린", "Theo", "하준",
        "Lena",
    ];
    Social {
        friends: names.iter().map(|name| person(name, "")).collect(),
        requests: vec![person("Noah", "")],
        blocked: 1,
        invite: Invite {
            url: "https://pickle.example/invite/sample".into(),
            left: 2,
            joined: vec![
                person("서연", "Joined 3 days ago"),
                person("Theo", "Joined today"),
            ],
            inviters: vec![person("Mina", "Joined 40 days ago")],
        },
    }
}

/// Made-up attachments: pictures over three days and two documents. No picture files exist,
/// so the tiles stay empty.
fn sample_archive() -> Vec<Attachment> {
    let day = 24. * 3600.;
    let now = Local::now().timestamp() as f64;
    let picture = |n: u32, days_ago: f64| Attachment {
        path: format!("attachments/sample/IMG_{n:04}.jpg"),
        seq: 1,
        at: now - days_ago * day,
        name: format!("IMG_{n:04}.jpg"),
        detail: String::new(),
        kind: AttachmentKind::Picture,
    };
    let file = |name: &str, detail: &str, days_ago: f64| Attachment {
        path: format!("attachments/sample/{name}"),
        seq: 2,
        at: now - days_ago * day,
        name: name.into(),
        detail: detail.into(),
        kind: AttachmentKind::File,
    };
    let mut attachments: Vec<Attachment> =
        (0..7).map(|n| picture(n, (n / 3) as f64 * 2.)).collect();
    attachments.push(file("모임 장소 비교.pdf", "PDF · 3 pages · 412 KB", 1.));
    attachments.push(file("budget.xlsx", "XLSX · 20 KB", 6.));
    attachments.sort_by(|a, b| b.at.total_cmp(&a.at));
    attachments
}

/// Made-up offers in the shape of the server's: three sections, one of which expands.
fn sample_explore() -> Explore {
    let offer = |id: &str, section: &str, area: &str, symbol: &str, title: &str, ask: &str| {
        Arc::new(Offer {
            id: id.into(),
            section: section.into(),
            title: title.into(),
            detail:
                "신청부터 결과 확인까지 제가 대신 챙겨 드릴게요. 필요한 확인은 그때그때 여쭤볼게요."
                    .into(),
            ask: ask.into(),
            picture: String::new(),
            symbol: symbol.into(),
            asked_by: "1,204 people asked for this".into(),
            steps: vec![
                "먼저 지금 상태를 확인해요".into(),
                "필요한 신청을 대신 넣어요. 본인 확인은 직접 해 주셔야 해요".into(),
                "결과를 정리해서 알려 드려요".into(),
            ],
            areas: vec![area.into()],
        })
    };
    let popular = vec![
        offer(
            "p1",
            "popular",
            "home",
            "house",
            "안 쓰는 사이트 계정을 정리해 드릴게요.",
            "안 쓰는 계정 지워줘",
        ),
        offer(
            "p2",
            "popular",
            "money",
            "banknote",
            "놓친 환급금을 찾아 드릴게요.",
            "환급금 찾아줘",
        ),
        offer(
            "p3",
            "popular",
            "travel",
            "airplane",
            "마일리지 좌석이 열리면 바로 알려 드릴게요.",
            "마일리지 좌석 찾아줘",
        ),
    ];
    let money = vec![
        offer(
            "m1",
            "money_back",
            "money",
            "banknote",
            "통신비 미환급금을 조회해 드릴게요.",
            "통신비 환급 조회해줘",
        ),
        offer(
            "m2",
            "money_back",
            "money",
            "doc.text",
            "연말정산 누락분을 확인해 드릴게요.",
            "연말정산 확인해줘",
        ),
        offer(
            "m3",
            "money_back",
            "health",
            "cross.case",
            "실손보험 청구를 대신 넣어 드릴게요.",
            "실손보험 청구해줘",
        ),
        offer(
            "m4",
            "money_back",
            "money",
            "banknote",
            "카드 포인트를 현금으로 바꿔 드릴게요.",
            "카드 포인트 바꿔줘",
        ),
        offer(
            "m5",
            "money_back",
            "shopping",
            "bag",
            "지연 배송 보상을 받아 드릴게요.",
            "배송 보상 받아줘",
        ),
    ];
    let trips = vec![
        offer(
            "t1",
            "travel",
            "travel",
            "ticket",
            "취소표가 나오면 바로 잡아 드릴게요.",
            "취소표 잡아줘",
        ),
        offer(
            "t2",
            "travel",
            "people",
            "person.2",
            "모임 날짜를 대신 조율해 드릴게요.",
            "모임 날짜 잡아줘",
        ),
    ];
    let ranked: Vec<Arc<Offer>> = popular
        .iter()
        .chain(&money)
        .chain(&trips)
        .cloned()
        .collect();
    Explore {
        areas: ["money", "shopping", "health", "people", "travel", "home"]
            .map(String::from)
            .to_vec(),
        popular: popular.iter().map(|offer| offer.id.clone()).collect(),
        more: money[3..].to_vec(),
        sections: vec![
            OfferSection {
                id: "popular".into(),
                title: "Popular lately".into(),
                offers: popular,
                preview: 3,
                expandable: false,
            },
            OfferSection {
                id: "money_back".into(),
                title: "Money you're owed".into(),
                offers: money,
                preview: 3,
                expandable: true,
            },
            OfferSection {
                id: "travel".into(),
                title: "Trips and flights".into(),
                offers: trips,
                preview: 2,
                expandable: false,
            },
        ],
        ranked,
    }
}

/// The app's own window, or with `float` one that stays above the others and takes no focus.
fn float_options(float: bool, cx: &mut App) -> WindowOptions {
    if !float {
        return ui::window_options(cx);
    }
    WindowOptions {
        kind: WindowKind::PopUp,
        focus: false,
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(1280.), px(140.)),
            size(px(440.), px(900.)),
        ))),
        ..Default::default()
    }
}

/// Plays one step of input into the window.
fn play(step: &str, window: &mut Window, cx: &mut App) {
    let (name, value) = step.split_once(':').unwrap_or((step, ""));
    match name {
        "key" => {
            window.dispatch_keystroke(Keystroke::parse(value).expect("a keystroke"), cx);
        }
        "type" => {
            for typed in value.chars().map(String::from) {
                let keystroke = Keystroke {
                    modifiers: Modifiers::none(),
                    key: typed.clone(),
                    key_char: Some(typed),
                };
                window.dispatch_keystroke(keystroke, cx);
            }
        }
        _ => panic!("unknown step {step}"),
    }
    window.refresh();
}

/// Stands in for the server's search: the bubbles that contain the text, newest first.
fn search(messages: &[Message], query: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    for message in messages.iter().rev() {
        for part in &message.parts {
            let Part::Bubble(bubble) = part else { continue };
            let line = bubble.text.replace('\n', " ");
            if let Some(at) = line.find(query) {
                results.push(SearchResult {
                    seq: Some(message.seq),
                    label: "Chat".into(),
                    date: "Nov 16".into(),
                    matches: vec![at..at + query.len(), line.len()..line.len()],
                    snippet: line,
                });
            }
        }
    }
    results
}

fn message(seq: u64, minutes_ago: f64, side: Side, text: &str) -> Message {
    // A room link has no label here; the sample has none to name.
    let parts = parse_message(text)
        .into_iter()
        .map(|block| match block {
            Block::RoomLink(room_id) => Part::RoomLink {
                room_id,
                label: "Open chat room".into(),
                picture: String::new(),
            },
            Block::Bubble(text) => Part::Bubble(Arc::new(text)),
            Block::Card(card) => Part::Card(Arc::new(card)),
        })
        .collect();
    Message {
        id: seq.to_string(),
        seq,
        at: Local::now().timestamp() as f64 - minutes_ago * 60.,
        side,
        parts,
    }
}

fn main() {
    let task_card = std::env::args().any(|arg| arg == "tasks");
    let mut messages = vec![
        message(
            1,
            200.,
            Side::Mine,
            "내일 전시 취소표 나오면 바로 잡아줘. 그리고 그 회사 사업자 정보도 확인해줘",
        ),
        message(
            2,
            199.,
            Side::Theirs,
            "<bubble>네! 링크 바로 확인해서 깔끔하게 정리해 드릴게요!</bubble>\n<bubble>취소표는 촉박한 날짜를 피해서, **오늘 기준 최소 일주일 뒤** 관람분부터 낚아채도록 감시 조건 바로 수정해 뒀어요!\n\n대량 오픈런은 취소표와 달리 날짜 상관없이 원래대로 10분 전 대기 유지할게요. (혹시 오픈런 작업 자체를 아예 빼라는 뜻이었다면 바로 지워드릴게요!)</bubble>\n```card\n## 🏢 샘플 투자조합\n### 사업자등록정보 조회\n- **상호** 샘플 투자조합\n- **대표자** (주)샘플인베스트먼트\n- **사업자번호** 000-00-00000\n- **사업장 주소** 서울특별시 중구 세종대로 110\n- **과세유형** 고유번호가 부여된 단체 (세금계산서 발행 불가)\n- **사업유형** 기타 개인 (업력 1개월)\n- **납세자 상태** 계속사업자\n\n[조회 페이지 열기](https://example.com/business)\n```\n<bubble>인원 정보나 매출·영업이익 같은 재무제표는 아직 등록된 내역이 없는 신규 결성 조합이에요.</bubble>",
        ),
        message(
            3,
            30.,
            Side::Theirs,
            "<bubble>내일 오전 10시 《샘플 콘서트》 취소표 1매 잡았어요!\n\n휴대폰으로 방금 도착한 인증번호 4자리만 적어주시면 바로 확정할게요.</bubble>",
        ),
    ];
    messages.push(Message {
        id: "request".into(),
        seq: 4,
        at: messages[2].at + 1.,
        side: Side::Theirs,
        parts: vec![Part::Request(Arc::new(Request {
            caption: "샘플 verification code".into(),
            question: None,
            options: vec![],
            status: RequestStatus::Expired,
        }))],
    });
    messages.push(message(5, 20., Side::Theirs, "<bubble>방금 건은 시간이 지나서 다시 매진으로 넘어갔어요! 취소표 감시는 계속 돌려둘게요.</bubble>"));
    messages.push(message(
        6,
        10.,
        Side::Mine,
        "The code card expired. Can you send a new code?",
    ));
    messages.push(message(7, 9., Side::Theirs, "<bubble>지금 자리 남아있는지 바로 확인해 볼게요!</bubble>\n<bubble>공식 웹사이트 (프로그램 상세)\nhttps://example.com/\n\n직접 둘러보셔도 되고, 폼 입력 귀찮으시면 언제든 말씀해 주세요.</bubble>"));
    messages.push(message(8, 1., Side::Mine, "링크 부탁해"));

    if std::env::args().any(|arg| arg == "links") {
        let value = serde_json::json!({
            "id": "sample-links", "role": "assistant", "seq": 9,
            "at": Local::now().timestamp() as f64,
            "text": "<bubble>[Open the sample report](downloads/sample.pdf)</bubble>\n```card\npickle://chat?room_id=11111111-1111-4111-8111-111111111111\n```",
            "app_references": {"pickle://chat?room_id=11111111-1111-4111-8111-111111111111": "Sample room"},
            "inline_files": {"downloads/sample.pdf": "attachments/2026-01-01/sample.pdf"},
            "file_meta": {"attachments/2026-01-01/sample.pdf": {"name": "Sample report.pdf", "size": 2048, "pages": 4}}
        });
        messages.push(openpickle_desktop::convert::message(
            &serde_json::from_value(value).unwrap(),
            None,
        ));
    }

    // `phone` adds a call Pickle placed, as the chat shows it: a card over its transcript.
    if std::env::args().any(|arg| arg == "phone") {
        messages.push(message(
            9,
            0.9,
            Side::Theirs,
            "<bubble>도서관에 바로 전화해서 운영 시간 확인해 볼게요!</bubble>",
        ));
        let task = serde_json::json!({
            "id": "sample-task", "role": "subagent", "seq": 10,
            "at": Local::now().timestamp() as f64 - 0.8 * 60.,
            "title": "📞 도서관 자동 응답 안내", "path": "handoffs/sample-call",
        });
        messages.push(openpickle_desktop::convert::message(
            &serde_json::from_value(task).unwrap(),
            None,
        ));
        messages.push(message(11, 0.5, Side::Mine, "주말에도 여는지 알려줘"));
    }

    // `tasks` adds a task that is not a call. The fake backend below has its card.
    if task_card {
        messages.push(message(
            9,
            0.9,
            Side::Theirs,
            "<bubble>I’ll compare a few options and put together a simple afternoon plan.</bubble>",
        ));
        let task = serde_json::json!({
            "id": "sample-task", "role": "subagent", "seq": 10,
            "at": Local::now().timestamp() as f64 - 0.8 * 60.,
            "title": "🧭 Plan a museum afternoon", "path": "handoffs/sample-task",
        });
        messages.push(openpickle_desktop::convert::message(
            &serde_json::from_value(task).unwrap(),
            None,
        ));
    }

    // `many` repeats the conversation to check that a long history costs nothing at launch.
    if std::env::args().any(|arg| arg == "many") {
        let sample = messages.clone();
        for round in 1..250u64 {
            messages.extend(sample.iter().cloned().map(|mut message| {
                message.seq += round * 100;
                message.id = message.seq.to_string();
                message
            }));
        }
    }
    let login = std::env::args().any(|arg| arg == "login");
    let whole_app = std::env::args().any(|arg| arg == "app");
    let phone_demo = std::env::args().any(|arg| arg == "phone-demo");
    let task_sheet = std::env::args().any(|arg| arg == "task-demo");
    let task_expanded = std::env::args().any(|arg| arg == "expanded");
    let task_plan = std::env::args().any(|arg| arg == "plan");
    // In the samples a call is live and a task runs, unless `ended` says otherwise.
    let live = !std::env::args().any(|arg| arg == "ended");
    let float = std::env::args().any(|arg| arg == "float");
    let steps: Vec<String> = std::env::args().filter(|arg| arg.contains(':')).collect();
    let started = std::time::Instant::now();
    Application::new()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            eprintln!("app ready after {:?}", started.elapsed());
            ui::bind_keys(cx);
            if login {
                cx.open_window(ui::window_options(cx), |window, cx| {
                    let view = cx.new(LoginView::new);
                    window.focus(&view.focus_handle(cx));
                    view
                })
                .unwrap();
            } else if phone_demo {
                let options = float_options(float, cx);
                cx.open_window(options, |window, cx| {
                    let view = cx.new(|cx| {
                        ui::phone::PhoneView::new(
                            ui::Identity::default(),
                            Some("sample-call".into()),
                            cx,
                        )
                    });
                    view.update(cx, |view, cx| {
                        view.set_call(
                            "sample-call",
                            Ok(sample_phone_call("sample-call", live)),
                            cx,
                        )
                    });
                    window.focus(&view.focus_handle(cx));
                    view
                })
                .unwrap();
            } else if task_sheet {
                let options = float_options(float, cx);
                cx.open_window(options, |window, cx| {
                    let view = cx.new(|cx| {
                        let pictures = cx.new(|_| ui::Pictures::default());
                        let path = "handoffs/sample-task".to_owned();
                        let mut view = ui::tasks::TaskView::new(
                            path.clone(),
                            sample_task_item(live),
                            pictures,
                            cx,
                        );
                        view.set_handoff(&path, Ok(sample_task_screen(live)), cx);
                        view.show(task_expanded, task_plan, cx);
                        view
                    });
                    window.focus(&view.focus_handle(cx));
                    view
                })
                .unwrap();
            } else if whole_app {
                let (commands, mut asked) = tokio::sync::mpsc::unbounded_channel();
                let (updates, received) = futures::channel::mpsc::unbounded();
                let conversation = Update::Conversation {
                    room_id: "primary".into(),
                    is_character: true,
                    can_send: true,
                    name: "Pickle".into(),
                    avatar: None,
                    messages: messages.clone(),
                };
                updates.unbounded_send(conversation).unwrap();
                let messages = messages.clone();
                std::thread::spawn(move || {
                    while let Some(command) = asked.blocking_recv() {
                        let update = match command {
                            Command::Search(query) => Update::Found {
                                results: Ok(search(&messages, &query)),
                                query,
                            },
                            Command::LoadSocial => Update::Social(Ok(sample_social())),
                            Command::LoadProfile => Update::Profile(Ok(sample_profile())),
                            Command::ResolvePhoneTask(path) => Update::PhoneTask {
                                path,
                                result: Ok("sample-call".into()),
                            },
                            Command::LoadPhoneCalls => Update::PhoneCalls(Ok(vec![
                                openpickle_desktop::phone::CallSummary {
                                    id: "sample-call".into(),
                                    title: "Library opening hours".into(),
                                    subtitle: "Pickle's phone call".into(),
                                    state: "running".into(),
                                    at: "Today".into(),
                                },
                            ])),
                            Command::LoadHandoffCard(path) if task_card => Update::HandoffCard {
                                path,
                                card: HandoffCard::Task(sample_task_item(live)),
                            },
                            Command::LoadHandoffCard(path) => Update::HandoffCard {
                                path,
                                card: HandoffCard::Phone(sample_phone_call("sample-call", live)),
                            },
                            Command::WatchHandoff(Some(path)) if task_card => Update::Handoff {
                                path,
                                result: Ok(sample_task_screen(live)),
                            },
                            Command::WatchHandoff(_) => continue,
                            Command::WatchPhoneCall(Some(call_id)) => Update::PhoneCall {
                                result: Ok(sample_phone_call(&call_id, live)),
                                call_id,
                            },
                            Command::WatchPhoneCall(None) => continue,
                            Command::EndPhoneCall { call_id, .. } => Update::PhoneCallEnded {
                                call_id,
                                result: Ok(()),
                            },
                            Command::LoadExplore => Update::Explore(Ok(sample_explore())),
                            Command::LoadArchive => Update::Archive {
                                room_id: "primary".into(),
                                attachments: sample_archive(),
                                complete: true,
                            },
                            Command::OpenRoom(room_id) => Update::Conversation {
                                room_id: if room_id.is_empty() {
                                    "primary".into()
                                } else {
                                    room_id.clone()
                                },
                                is_character: room_id.is_empty(),
                                can_send: room_id.is_empty(),
                                name: if room_id.is_empty() {
                                    "Pickle".into()
                                } else {
                                    "Sample room".into()
                                },
                                avatar: None,
                                messages: if room_id.is_empty() {
                                    messages.clone()
                                } else {
                                    vec![message(
                                        1,
                                        1.,
                                        Side::Theirs,
                                        "<bubble>Sample conversation.</bubble>",
                                    )]
                                },
                            },
                            Command::PreviewAttachment(path) => Update::AttachmentPreview {
                                path,
                                result: Err("No thumbnail exists for this sample file.".into()),
                            },
                            _ => continue,
                        };
                        updates.unbounded_send(update).ok();
                    }
                });
                let options = float_options(float, cx);
                // The untyped handle: the typed one would hold the root view while a key is
                // played, and the keys make the root view do things.
                let window: AnyWindowHandle = cx
                    .open_window(options, |window, cx| {
                        cx.new(|cx| Shell::new(commands, received, window, cx))
                    })
                    .unwrap()
                    .into();
                cx.spawn(async move |cx: &mut AsyncApp| {
                    for step in steps {
                        let pause = step
                            .strip_prefix("wait:")
                            .map_or(300, |ms| ms.parse().unwrap());
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(pause))
                            .await;
                        if !step.starts_with("wait:") {
                            window
                                .update(cx, |_, window, cx| play(&step, window, cx))
                                .ok();
                        }
                    }
                })
                .detach();
            } else {
                cx.open_window(ui::window_options(cx), |window, cx| {
                    let view = cx.new(|cx| {
                        let pictures = cx.new(|_| ui::Pictures::default());
                        let mut view = ChatView::new(pictures, cx);
                        let (rows, _) = build_rows(&messages, Some(Some("Working")), Local::now());
                        eprintln!("{} messages, {} rows", messages.len(), rows.len());
                        view.set_rows(rows, cx);
                        view
                    });
                    window.focus(&view.focus_handle(cx));
                    // GPUI draws the first frame while opening the window; this runs when the
                    // display asks for the next one.
                    window.on_next_frame(move |_, _| {
                        eprintln!("second frame requested after {:?}", started.elapsed())
                    });
                    view
                })
                .unwrap();
                eprintln!(
                    "window open and first frame drawn after {:?}",
                    started.elapsed()
                );
            }
            if !float {
                cx.activate(true);
            }
        });
}
