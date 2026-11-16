//! Selecting text in the conversation with the mouse and copying it.
//!
//! A selection runs from one position to another in reading order and may span many messages.
//! A position names a row of the list, one of the texts that row shows, and a byte offset in
//! it, so it stays meaningful for rows that are not on screen: copying reads the text from the
//! rows themselves, not from what is drawn.

use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, StyledText, TextLayout, Window, px,
};

use crate::model::Row;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub row: usize,
    /// Which of the row's texts, counted as [`Row::texts`] lists them.
    pub text: usize,
    /// Byte offset in that text.
    pub index: usize,
}

/// How far the pointer is from a text: down the screen first, then across. A text the pointer is
/// level with comes before one above or below it.
type Distance = (Pixels, Pixels);

#[derive(Default)]
pub struct Selection {
    /// Where the press started, and where the pointer is now.
    anchor: Option<Position>,
    head: Option<Position>,
    /// The left button is down and moving it extends the selection.
    dragging: bool,
    /// A text took the press that is being handled, so the list must not treat it as a press
    /// on empty space.
    pressed_on_text: bool,
    /// The nearest text that claimed the pointer since it last moved.
    nearest: Option<Distance>,
    /// The drag began on empty space and the pointer has moved once since. The drag starts at
    /// the text nearest to that first position, which is known when every text has claimed it.
    start_follows: bool,
    /// The list is scrolling under a pointer that holds still, so texts claim the pointer as
    /// they are drawn.
    scrolling: bool,
    /// The row under the pointer at the last right click.
    context_row: Option<usize>,
}

pub type SharedSelection = Rc<RefCell<Selection>>;

impl Selection {
    /// The selected stretch in reading order, or `None` when nothing is selected.
    fn span(&self) -> Option<(Position, Position)> {
        let (anchor, head) = (self.anchor?, self.head?);
        (anchor != head).then(|| (anchor.min(head), anchor.max(head)))
    }

    pub fn is_empty(&self) -> bool {
        self.span().is_none()
    }

    /// Whether a drag is under way that has a place to start from.
    pub fn is_extending(&self) -> bool {
        self.dragging && self.anchor.is_some()
    }

    /// The part of one text that is selected. `len` is the length of that text.
    pub fn range_in(&self, row: usize, text: usize, len: usize) -> Option<Range<usize>> {
        let (start, end) = self.span()?;
        let here = (row, text);
        if here < (start.row, start.text) || here > (end.row, end.text) {
            return None;
        }
        let from = if here == (start.row, start.text) {
            start.index.min(len)
        } else {
            0
        };
        let to = if here == (end.row, end.text) {
            end.index.min(len)
        } else {
            len
        };
        (from < to).then_some(from..to)
    }

    /// The selected text, each text on its own line. It is read from the rows, so it does not
    /// matter which of them are on screen.
    pub fn text(&self, rows: &[Row]) -> Option<String> {
        let (start, end) = self.span()?;
        let mut lines: Vec<&str> = Vec::new();
        for (row_index, row) in rows.iter().enumerate().take(end.row + 1).skip(start.row) {
            for (text_index, text) in row.texts().into_iter().enumerate() {
                if let Some(range) = self.range_in(row_index, text_index, text.len())
                    && let Some(part) = text.get(range)
                {
                    lines.push(part);
                }
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    pub fn select_all(&mut self, rows: &[Row]) {
        let ends = |(row, content): (usize, &Row)| {
            let texts = content.texts();
            let last = texts.last()?;
            Some((
                Position {
                    row,
                    text: 0,
                    index: 0,
                },
                Position {
                    row,
                    text: texts.len() - 1,
                    index: last.len(),
                },
            ))
        };
        let first = rows.iter().enumerate().find_map(ends);
        let last = rows.iter().enumerate().rev().find_map(ends);
        self.anchor = first.map(|(start, _)| start);
        self.head = last.map(|(_, end)| end);
        self.dragging = false;
    }

    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
    }

    /// The rows from `unchanged` on were replaced. A selection that reaches them no longer
    /// names the text it was made on, so it is dropped.
    pub fn rows_changed(&mut self, unchanged: usize) {
        if self
            .anchor
            .max(self.head)
            .is_some_and(|last| last.row >= unchanged)
        {
            self.clear();
            self.dragging = false;
        }
    }

    /// The left button went down on a text.
    fn press(&mut self, at: Position) {
        self.anchor = Some(at);
        self.head = Some(at);
        self.dragging = true;
        self.pressed_on_text = true;
    }

    /// The left button went down somewhere in the list. Returns whether that changed what is
    /// selected. A press on empty space drops the selection but still starts a drag, which
    /// begins at the first text the pointer comes near.
    pub fn press_anywhere(&mut self) -> bool {
        if std::mem::take(&mut self.pressed_on_text) {
            return false;
        }
        let changed = !self.is_empty();
        self.clear();
        self.dragging = true;
        changed
    }

    pub fn release(&mut self) {
        self.dragging = false;
    }

    /// The pointer moved. The texts on screen claim it next, in any order.
    fn begin_move(&mut self, button_down: bool) {
        self.nearest = None;
        self.dragging &= button_down;
        self.start_follows = self.anchor.is_none();
    }

    /// The list scrolls, or stopped scrolling, under a pointer that holds a drag.
    pub fn set_scrolling(&mut self, scrolling: bool) {
        self.scrolling = scrolling;
        self.nearest = None;
    }

    /// Whether a text this far from the pointer would be the nearest so far.
    fn wants(&self, distance: Distance) -> bool {
        self.dragging && self.nearest.is_none_or(|nearest| distance < nearest)
    }

    /// A text claims the pointer, which is at `at` in it. Returns whether the selection changed.
    fn drag_to(&mut self, at: Position, distance: Distance) -> bool {
        if !self.wants(distance) {
            return false;
        }
        self.nearest = Some(distance);
        let anchor = if self.start_follows {
            at
        } else {
            self.anchor.unwrap_or(at)
        };
        let changed = (Some(anchor), Some(at)) != (self.anchor, self.head);
        self.anchor = Some(anchor);
        self.head = Some(at);
        changed
    }

    /// Whether the last press ended where it began, which makes it a click.
    fn is_click(&self) -> bool {
        self.anchor == self.head
    }

    pub fn set_context_row(&mut self, row: usize) {
        self.context_row = Some(row);
    }

    /// The row that was right-clicked, once.
    pub fn take_context_row(&mut self) -> Option<usize> {
        self.context_row.take()
    }
}

/// Text that can be selected with the mouse, and whose links can be clicked.
pub struct SelectableText {
    pub text: StyledText,
    /// What `text` shows. Offsets in it are offsets in the row's text.
    pub content: SharedString,
    pub row: usize,
    /// Position of this text in its row's [`Row::texts`].
    pub ordinal: usize,
    pub selection: SharedSelection,
    pub links: Vec<(Range<usize>, SharedString)>,
    pub on_link: fn(&str, &mut Window, &mut App),
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// How far `value` is outside `from..to`.
fn outside(value: Pixels, from: Pixels, to: Pixels) -> Pixels {
    (from - value).max(value - to).max(px(0.))
}

/// The offset of the character boundary nearest to `point`.
fn offset_at(layout: &TextLayout, content: &str, point: Point<Pixels>) -> usize {
    match layout.index_for_position(point) {
        Ok(index) => {
            // `index` is the character under the point. Past its middle, its far side is nearer.
            let next = content[index..]
                .chars()
                .next()
                .map_or(index, |c| index + c.len_utf8());
            match (
                layout.position_for_index(index),
                layout.position_for_index(next),
            ) {
                (Some(near), Some(far))
                    if far.y == near.y && point.x - near.x > (far.x - near.x) / 2. =>
                {
                    next
                }
                _ => index,
            }
        }
        // Beside the text: the end of the line the point is level with.
        Err(index) => index,
    }
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.text
            .prepaint(None, inspector_id, bounds, state, window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text
            .paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);

        let view = window.current_view();
        let (row, ordinal) = (self.row, self.ordinal);
        let position_at = {
            let (layout, content) = (self.text.layout().clone(), self.content.clone());
            move |point| Position {
                row,
                text: ordinal,
                index: offset_at(&layout, &content, point),
            }
        };
        // Extends a drag to `point` if no other text is nearer to it.
        let claim = {
            let (selection, position_at) = (self.selection.clone(), position_at.clone());
            move |point: Point<Pixels>| {
                let distance = (
                    outside(point.y, bounds.top(), bounds.bottom()),
                    outside(point.x, bounds.left(), bounds.right()),
                );
                let wanted = selection.borrow().wants(distance);
                wanted && selection.borrow_mut().drag_to(position_at(point), distance)
            }
        };
        let link_at = {
            let (layout, links) = (self.text.layout().clone(), std::mem::take(&mut self.links));
            move |point| {
                let index = layout.index_for_position(point).ok()?;
                links
                    .iter()
                    .find(|(range, _)| range.contains(&index))
                    .map(|(_, url)| url.clone())
            }
        };

        let over_link = hitbox.is_hovered(window) && link_at(window.mouse_position()).is_some();
        window.set_cursor_style(
            if over_link {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            },
            hitbox,
        );
        if self.selection.borrow().scrolling {
            // The next frame shows the result; scrolling asks for one.
            claim(window.mouse_position());
        }

        window.on_mouse_event({
            let (hitbox, selection) = (hitbox.clone(), self.selection.clone());
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble
                    && hitbox.is_hovered(window)
                    && event.button == MouseButton::Left
                {
                    selection.borrow_mut().press(position_at(event.position));
                    cx.notify(view);
                }
            }
        });

        window.on_mouse_event({
            let selection = self.selection.clone();
            move |event: &MouseMoveEvent, phase, _, cx| match phase {
                // Every text hears of the move before any of them claims the pointer.
                DispatchPhase::Capture => selection.borrow_mut().begin_move(event.dragging()),
                DispatchPhase::Bubble => {
                    if claim(event.position) {
                        cx.notify(view);
                    }
                }
            }
        });

        window.on_mouse_event({
            let (hitbox, selection, on_link) =
                (hitbox.clone(), self.selection.clone(), self.on_link);
            move |event: &MouseUpEvent, phase, window, cx| {
                let released_here = phase == DispatchPhase::Bubble
                    && event.button == MouseButton::Left
                    && hitbox.is_hovered(window);
                // A press that did not drag is a click, which follows a link.
                if released_here
                    && selection.borrow().is_click()
                    && let Some(url) = link_at(event.position)
                {
                    on_link(&url, window, cx);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        content::RichText,
        model::{Part, RowKind, Side},
    };

    fn bubble(text: &str) -> Row {
        let part = Part::Bubble(Arc::new(RichText {
            text: text.into(),
            spans: vec![],
        }));
        Row {
            kind: RowKind::Part(part),
            side: Side::Theirs,
            joined_above: false,
            joined_below: false,
        }
    }

    fn at(row: usize, index: usize) -> Position {
        Position {
            row,
            text: 0,
            index,
        }
    }

    const ON_IT: Distance = (px(0.), px(0.));

    /// The pointer moves onto the text at `at`.
    fn move_to(selection: &mut Selection, at: Position) -> bool {
        selection.begin_move(true);
        selection.drag_to(at, ON_IT)
    }

    #[test]
    fn a_drag_selects_across_rows_in_either_direction() {
        let rows = [bubble("첫 번째 말"), bubble("가운데"), bubble("마지막 말")];
        let mut selection = Selection::default();
        selection.press(at(0, 4));
        assert!(selection.is_empty());
        assert!(move_to(&mut selection, at(2, 9)));
        assert_eq!(
            selection.text(&rows).as_deref(),
            Some("번째 말\n가운데\n마지막")
        );
        assert_eq!(
            selection.range_in(0, 0, rows[0].texts()[0].len()),
            Some(4..14)
        );
        assert_eq!(selection.range_in(1, 0, 9), Some(0..9));
        assert_eq!(selection.range_in(2, 0, 13), Some(0..9));

        // Dragging back above the start selects the other way.
        move_to(&mut selection, at(0, 0));
        assert_eq!(selection.text(&rows).as_deref(), Some("첫 "));

        selection.release();
        assert!(
            !move_to(&mut selection, at(2, 0)),
            "moving without the button down changes nothing"
        );
    }

    #[test]
    fn the_text_nearest_to_the_pointer_takes_the_drag() {
        let rows = [bubble("위"), bubble("아래")];
        let mut selection = Selection::default();
        selection.press(at(0, 0));
        selection.begin_move(true);
        // Texts claim the pointer in drawing order; the one it is level with wins either way.
        assert!(selection.drag_to(at(0, 3), (px(30.), px(0.))));
        assert!(selection.drag_to(at(1, 3), (px(0.), px(12.))));
        assert!(!selection.drag_to(at(0, 3), (px(30.), px(0.))));
        assert_eq!(selection.text(&rows).as_deref(), Some("위\n아"));

        // A move event that says the button is up ends the drag, even if the release was missed.
        selection.begin_move(false);
        assert!(!selection.drag_to(at(1, 6), ON_IT));
    }

    #[test]
    fn select_all_copies_every_row_even_those_not_drawn() {
        let rows: Vec<Row> = (0..500).map(|n| bubble(&format!("말 {n}"))).collect();
        let mut selection = Selection::default();
        selection.select_all(&rows);
        let text = selection.text(&rows).unwrap();
        assert_eq!(text.lines().count(), 500);
        assert!(text.starts_with("말 0\n") && text.ends_with("\n말 499"));

        selection.select_all(&[]);
        assert!(selection.is_empty());
    }

    #[test]
    fn a_press_on_empty_space_drops_the_selection_and_can_start_a_drag() {
        let rows = [bubble("하나"), bubble("둘")];
        let mut selection = Selection::default();
        selection.press(at(0, 0));
        // The list sees the same press after the text did, and leaves it alone.
        assert!(!selection.press_anywhere());
        move_to(&mut selection, at(1, 3));
        selection.release();
        assert!(!selection.is_empty());

        assert!(selection.press_anywhere());
        assert!(selection.is_empty());
        // The drag starts at the text nearest to where the pointer first moved, whichever
        // text claims that position first.
        selection.begin_move(true);
        selection.drag_to(at(1, 0), (px(40.), px(0.)));
        selection.drag_to(at(0, 3), ON_IT);
        move_to(&mut selection, at(1, 3));
        assert_eq!(selection.text(&rows).as_deref(), Some("나\n둘"));
    }

    #[test]
    fn a_selection_survives_new_rows_but_not_a_change_to_its_own() {
        let mut selection = Selection::default();
        selection.press(at(1, 0));
        move_to(&mut selection, at(2, 3));
        selection.rows_changed(3);
        assert!(!selection.is_empty());
        selection.rows_changed(2);
        assert!(selection.is_empty() && !selection.is_extending());
    }
}
