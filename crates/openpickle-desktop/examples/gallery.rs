//! Opens the chat screen with made-up messages that have the same structure as the Figma frames,
//! to compare the two side by side: `cargo run -p openpickle-desktop --example gallery`.
//! Pass `login` to see the sign-in screen instead.

use std::sync::Arc;

use chrono::Local;
use gpui::{App, AppContext, Application, Focusable};
use openpickle_desktop::{
    assets::Assets,
    content::{Block, parse_message},
    model::{Message, Part, Request, RequestStatus, Side, build_rows},
    ui::{self, chat::ChatView, login::LoginView},
};

fn message(seq: u64, minutes_ago: f64, side: Side, text: &str) -> Message {
    let parts = parse_message(text)
        .into_iter()
        .map(|block| match block {
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
    let started = std::time::Instant::now();
    Application::new()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            eprintln!("app ready after {:?}", started.elapsed());
            ui::input::bind_keys(cx);
            if login {
                cx.open_window(ui::window_options(cx), |window, cx| {
                    let view = cx.new(LoginView::new);
                    window.focus(&view.focus_handle(cx));
                    view
                })
                .unwrap();
            } else {
                cx.open_window(ui::window_options(cx), |window, cx| {
                    let view = cx.new(|cx| {
                        let mut view = ChatView::new(cx);
                        let rows = build_rows(&messages, Some(Some("Working")), Local::now());
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
            cx.activate(true);
        });
}
