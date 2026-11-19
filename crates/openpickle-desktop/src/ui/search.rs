//! The search screen: the field at the bottom, what it found above.

use std::time::Duration;

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle, SharedString,
    Stateful, StyledText, Subscription, Task, Window, div, prelude::*, px,
};

use crate::{
    model::SearchResult,
    theme,
    ui::{
        Cancel, Next, Previous, chrome, icon,
        input::{InputEvent, TextInput},
    },
};

pub enum SearchEvent {
    Edited,
    /// Search the messages for this text.
    Query(String),
    /// Open the message with this sequence number.
    Open(u64),
    Close,
}

enum State {
    /// The field is empty.
    Idle,
    Searching,
    Found,
    Failed(SharedString),
}

pub struct SearchView {
    input: Entity<TextInput>,
    /// Loading or failure for an individual result. Other results remain available.
    opening: Option<SharedString>,
    /// What the last finished search found.
    results: Vec<SearchResult>,
    /// The result the arrow keys are on, which Enter opens.
    chosen: Option<usize>,
    state: State,
    /// Waits for a pause in the typing, then searches.
    pause: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SearchEvent> for SearchView {}

impl SearchView {
    /// How long the typing must pause before a search goes out.
    const PAUSE: Duration = Duration::from_millis(250);

    pub fn new(cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Search", cx));
        let subscriptions = vec![cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Changed => {
                this.results.clear();
                this.chosen = None;
                this.opening = None;
                this.state = if this.query(cx).is_empty() {
                    State::Idle
                } else {
                    State::Searching
                };
                cx.emit(SearchEvent::Edited);
                this.pause = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Self::PAUSE).await;
                    this.update(cx, |this, cx| this.search(cx)).ok();
                }));
                // The clear button appears and disappears with the text.
                cx.notify();
            }
            // Enter opens the result the arrow keys are on. Without one it searches at once.
            InputEvent::Submitted(_) => {
                match this.chosen.and_then(|chosen| this.results.get(chosen)?.seq) {
                    Some(seq) => cx.emit(SearchEvent::Open(seq)),
                    None => this.search(cx),
                }
            }
            InputEvent::Selected => {}
        })];
        Self {
            input,
            opening: None,
            results: Vec::new(),
            chosen: None,
            state: State::Idle,
            pause: None,
            _subscriptions: subscriptions,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).text().trim().to_owned()
    }

    /// Moves the choice of the arrow keys up or down the results.
    fn choose(&mut self, step: isize, cx: &mut Context<Self>) {
        let last = self.results.len().checked_sub(1);
        self.chosen = match (self.chosen, last) {
            (_, None) => None,
            (None, Some(_)) => (step > 0).then_some(0),
            (Some(chosen), Some(last)) => Some(chosen.saturating_add_signed(step).min(last)),
        };
        cx.notify();
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        self.pause = None;
        let query = self.query(cx);
        self.results.clear();
        self.chosen = None;
        self.opening = None;
        if query.is_empty() {
            self.state = State::Idle;
        } else {
            self.state = State::Searching;
            cx.emit(SearchEvent::Query(query));
        }
        cx.notify();
    }

    /// Takes in the current search results. The shell matches the request generation first.
    pub fn found(&mut self, results: Result<Vec<SearchResult>, String>, cx: &mut Context<Self>) {
        self.chosen = None;
        match results {
            Ok(results) => {
                self.results = results;
                self.state = State::Found;
            }
            Err(reason) => {
                self.results.clear();
                self.state = State::Failed(reason.into());
            }
        }
        cx.notify();
    }

    pub fn set_opening(&mut self, note: SharedString, cx: &mut Context<Self>) {
        self.opening = Some(note);
        cx.notify();
    }

    pub fn reopen(&mut self, cx: &mut Context<Self>) {
        self.opening = None;
        if matches!(self.state, State::Searching) {
            self.search(cx);
        } else {
            cx.notify();
        }
    }

    fn result(
        index: usize,
        result: &SearchResult,
        chosen: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let matching = HighlightStyle {
            color: Some(theme::color(theme::LINK)),
            ..Default::default()
        };
        let snippet = StyledText::new(SharedString::from(result.snippet.clone()))
            .with_highlights(result.matches.iter().map(|range| (range.clone(), matching)));
        let row = div()
            .id(("result", index))
            .px(px(12.))
            .pt(px(15.))
            .pb(px(13.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .border_b_1()
            .border_color(theme::alpha(theme::HAIRLINE, 0.6))
            .when(chosen, |row| row.bg(theme::alpha(0xffffff, 0.07)))
            .child(
                div()
                    .truncate()
                    .text_size(theme::LABEL)
                    .line_height(theme::LABEL_LINE)
                    .child(snippet),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(14.))
                    .line_height(px(18.))
                    .text_color(theme::color(theme::TEXT_FAINT))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(icon("message-circle", 16., theme::TEXT_FAINT))
                            .child(result.label.clone()),
                    )
                    .child(result.date.clone()),
            );
        match result.seq {
            Some(seq) => row
                .cursor_pointer()
                .hover(|row| row.bg(theme::alpha(0xffffff, 0.04)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.results.get(index).and_then(|result| result.seq) == Some(seq) {
                        cx.emit(SearchEvent::Open(seq));
                    }
                })),
            None => row,
        }
    }
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let note = match &self.state {
            State::Idle => Some("Search your messages".into()),
            State::Failed(reason) => Some(reason.clone()),
            State::Found if self.results.is_empty() => Some("No results".into()),
            State::Searching => Some("Searching…".into()),
            State::Found => None,
        };
        let results = div()
            .id("results")
            .flex_1()
            .min_h_0()
            .w_full()
            .max_w(theme::COLUMN_MAX)
            .px(px(12.))
            .pt(px(44.))
            .overflow_y_scroll();
        let results = match note {
            Some(note) => results.child(
                div()
                    .pt(px(120.))
                    .px(px(24.))
                    .flex()
                    .justify_center()
                    .text_center()
                    .text_color(theme::color(theme::TEXT_FAINT))
                    .child(note),
            ),
            None => {
                let rows: Vec<_> = self
                    .results
                    .iter()
                    .enumerate()
                    .map(|(index, result)| {
                        Self::result(index, result, self.chosen == Some(index), cx)
                    })
                    .collect();
                results.children(rows)
            }
        };

        let has_text = !self.input.read(cx).text().is_empty();
        let field = chrome("search-field")
            .flex_1()
            .min_w_0()
            .h(px(53.))
            .rounded_full()
            .pl(px(17.))
            .pr(px(12.))
            .gap(px(11.))
            .justify_start()
            .child(icon("search", 22., theme::TEXT))
            .child(self.input.clone())
            .when(has_text, |field| {
                field.child(
                    div()
                        .id("clear")
                        .cursor_pointer()
                        .child(icon("circle-x", 21., theme::TEXT_FAINT))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.input.update(cx, |input, cx| input.set_text("", cx));
                            this.search(cx);
                        })),
                )
            });
        let close = chrome("close")
            .size(px(53.))
            .flex_none()
            .rounded_full()
            .cursor_pointer()
            .child(icon("x", 24., theme::TEXT))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Close)));

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(theme::color(theme::PAGE))
            .text_color(theme::color(theme::TEXT))
            .font_family(theme::FONT)
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE)
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(SearchEvent::Close)))
            .on_action(cx.listener(|this, _: &Previous, _, cx| this.choose(-1, cx)))
            .on_action(cx.listener(|this, _: &Next, _, cx| this.choose(1, cx)))
            .flex()
            .flex_col()
            .items_center()
            .child(results)
            .when_some(self.opening.clone(), |view, note| {
                view.child(
                    div()
                        .w_full()
                        .max_w(theme::COLUMN_MAX)
                        .px(px(20.))
                        .py(px(8.))
                        .text_size(theme::LABEL)
                        .text_color(theme::color(theme::TEXT_FAINT))
                        .child(note),
                )
            })
            .child(
                div()
                    .w_full()
                    .max_w(theme::COLUMN_MAX)
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(20.))
                    .pt(px(10.))
                    .pb(px(21.))
                    .child(field)
                    .child(close),
            )
    }
}

impl Focusable for SearchView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::TestAppContext;

    use super::*;

    fn result(seq: u64) -> SearchResult {
        SearchResult {
            seq: Some(seq),
            label: "Chat".into(),
            date: "Nov 16".into(),
            snippet: "내일 비 온대요".into(),
            matches: vec![0..6, 7..10],
        }
    }

    #[gpui::test]
    fn search_debounces_typing_and_opens_a_visible_result(cx: &mut TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view = SearchView::new(cx);
            window.focus(&view.focus_handle(cx));
            view
        });
        let asked = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let asked = asked.clone();
            cx.subscribe(&view, move |_, event, _| {
                if let SearchEvent::Query(query) = event {
                    asked.borrow_mut().push(query.clone());
                }
            })
            .detach();
        });

        cx.simulate_input("비");
        cx.simulate_input("가");
        assert!(asked.borrow().is_empty(), "nothing goes out while typing");
        cx.executor().advance_clock(SearchView::PAUSE);
        cx.run_until_parked();
        assert_eq!(*asked.borrow(), ["비가"]);

        view.read_with(cx, |view, _| assert!(view.results.is_empty()));

        // The arrow keys choose a result and Enter opens it.
        let opened = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let opened = opened.clone();
            cx.subscribe(&view, move |_, event, _| {
                if let SearchEvent::Open(seq) = event {
                    opened.borrow_mut().push(*seq);
                }
            })
            .detach();
        });
        view.update(cx, |view, cx| {
            view.found(Ok(vec![result(3), result(2), result(1)]), cx)
        });
        cx.simulate_keystrokes("down down down down up enter");
        assert_eq!(*opened.borrow(), [2]);

        // Thirty results do not fit the window; the list scrolls, and the first row is then
        // another result.
        cx.simulate_resize(gpui::size(gpui::px(440.), gpui::px(900.)));
        view.update(cx, |view, cx| {
            let thirty = (1..=30).map(result).collect();
            view.found(Ok(thirty), cx)
        });
        cx.run_until_parked();
        let first_row = gpui::point(gpui::px(120.), gpui::px(75.));
        cx.simulate_click(first_row, gpui::Modifiers::none());
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(gpui::px(220.), gpui::px(400.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-1500.))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();
        cx.simulate_click(first_row, gpui::Modifiers::none());
        let opened_now = opened.borrow()[1..].to_vec();
        assert_eq!(opened_now.len(), 2);
        assert!(
            opened_now[0] == 1 && opened_now[1] > 1,
            "opened {opened_now:?}"
        );

        // Emptying the field empties the screen without asking the server.
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| input.set_text("", cx));
            view.search(cx);
        });
        view.read_with(cx, |view, _| assert!(view.results.is_empty()));
        assert_eq!(asked.borrow().len(), 1);
    }
}
