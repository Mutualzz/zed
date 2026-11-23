use std::ops::Range;
use std::time::Duration;

use collections::FxHashMap;

use crate::{
    App, Bounds, CursorStyle, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Rgba, ScrollDelta, ScrollEvent, ScrollSource, Task, TextLayout, TouchPhase,
    Window, point,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct TextKey {
    path: u64,
    ordinal: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct TextPoint {
    key: TextKey,
    index: usize,
}

#[derive(Clone)]
pub(crate) struct SelectableText {
    pub(crate) key: TextKey,
    pub(crate) layout: TextLayout,
    pub(crate) mask: Bounds<Pixels>,
    pub(crate) hitbox_floor: usize,
    pub(crate) selectable: bool,
}

impl SelectableText {
    fn visible_bounds(&self) -> Bounds<Pixels> {
        self.layout.bounds().intersect(&self.mask)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Granularity {
    #[default]
    Character,
    Word,
    Block,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct SelectedPiece {
    key: TextKey,
    range: Range<usize>,
    text: String,
}

pub(crate) struct TextSelection {
    anchor: Option<TextPoint>,
    head: Option<TextPoint>,
    initial: Option<(TextPoint, TextPoint)>,
    granularity: Granularity,
    dragging: bool,
    suppressed: bool,
    scroll_region: Option<Bounds<Pixels>>,
    autoscrolling: bool,
    autoscroll_task: Option<Task<()>>,
    resolved: FxHashMap<TextKey, Range<usize>>,
    pieces: Vec<SelectedPiece>,
    pub(crate) color: Hsla,
}

impl Default for TextSelection {
    fn default() -> Self {
        Self {
            anchor: None,
            head: None,
            initial: None,
            granularity: Granularity::Character,
            dragging: false,
            suppressed: false,
            scroll_region: None,
            autoscrolling: false,
            autoscroll_task: None,
            resolved: FxHashMap::default(),
            pieces: Vec::new(),
            color: Rgba {
                r: 0.0,
                g: 65.0 / 255.0,
                b: 198.0 / 255.0,
                a: 0.8,
            }
            .into(),
        }
    }
}

const NEWLINE_EXTENSION_EM: f32 = 0.25;
const AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(16);
const AUTOSCROLL_SPEED_RATIO: f32 = 0.5;
const AUTOSCROLL_MIN_STEP: f32 = 2.0;
const AUTOSCROLL_MAX_STEP: f32 = 40.0;

fn autoscroll_step(before: Pixels, after: Pixels) -> f32 {
    let distance = if before > Pixels::ZERO {
        f32::from(before)
    } else if after > Pixels::ZERO {
        -f32::from(after)
    } else {
        return 0.0;
    };
    (distance.abs() * AUTOSCROLL_SPEED_RATIO).clamp(AUTOSCROLL_MIN_STEP, AUTOSCROLL_MAX_STEP)
        * distance.signum()
}

pub(crate) fn autoscroll_delta(region: Bounds<Pixels>, position: Point<Pixels>) -> Point<Pixels> {
    point(
        Pixels(autoscroll_step(
            region.left() - position.x,
            position.x - region.right(),
        )),
        Pixels(autoscroll_step(
            region.top() - position.y,
            position.y - region.bottom(),
        )),
    )
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn character_class(character: char) -> u8 {
    if is_word_character(character) {
        0
    } else if character.is_whitespace() {
        1
    } else {
        2
    }
}

pub(crate) fn word_range(text: &str, index: usize) -> Range<usize> {
    let index = floor_char_boundary(text, index.min(text.len()));
    let Some(character) = text[index..]
        .chars()
        .next()
        .or_else(|| text[..index].chars().next_back())
    else {
        return index..index;
    };
    if character == '\n' {
        return index..index;
    }
    let class = character_class(character);
    let start = text[..index]
        .char_indices()
        .rev()
        .take_while(|(_, character)| *character != '\n' && character_class(*character) == class)
        .last()
        .map_or(index, |(position, _)| position);
    let end = text[index..]
        .char_indices()
        .take_while(|(_, character)| *character != '\n' && character_class(*character) == class)
        .last()
        .map_or(index, |(position, character)| {
            index + position + character.len_utf8()
        });
    start..end
}

pub(crate) fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ordered(first: TextPoint, second: TextPoint, order: &FxHashMap<TextKey, usize>) -> bool {
    let first_position = order.get(&first.key).copied().unwrap_or(0);
    let second_position = order.get(&second.key).copied().unwrap_or(0);
    (first_position, first.index) <= (second_position, second.index)
}

impl TextSelection {
    pub(crate) fn range_for(&self, key: TextKey) -> Option<Range<usize>> {
        self.resolved
            .get(&key)
            .filter(|range| !range.is_empty())
            .cloned()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pieces.iter().all(|piece| piece.range.is_empty())
    }

    fn clear(&mut self) -> bool {
        let had_selection = !self.is_empty() || self.anchor.is_some();
        self.anchor = None;
        self.head = None;
        self.initial = None;
        self.stop_dragging();
        self.resolved.clear();
        self.pieces.clear();
        had_selection
    }

    pub(crate) fn stop_dragging(&mut self) {
        self.dragging = false;
        self.scroll_region = None;
        self.autoscrolling = false;
        self.autoscroll_task = None;
    }

    pub(crate) fn resolve(&mut self, texts: &[SelectableText]) {
        let (Some(anchor), Some(head)) = (self.anchor, self.head) else {
            self.resolved.clear();
            self.pieces.clear();
            return;
        };
        let order: FxHashMap<TextKey, usize> = texts
            .iter()
            .enumerate()
            .map(|(position, text)| (text.key, position))
            .collect();
        let (Some(&anchor_position), Some(&head_position)) =
            (order.get(&anchor.key), order.get(&head.key))
        else {
            self.resolved = self
                .pieces
                .iter()
                .filter(|piece| order.contains_key(&piece.key))
                .map(|piece| (piece.key, piece.range.clone()))
                .collect();
            return;
        };
        let (start, end) = if (anchor_position, anchor.index) <= (head_position, head.index) {
            (anchor, head)
        } else {
            (head, anchor)
        };
        let start_position = anchor_position.min(head_position);
        let end_position = anchor_position.max(head_position);
        self.resolved.clear();
        self.pieces.clear();
        for text in &texts[start_position..=end_position] {
            if !text.selectable {
                continue;
            }
            let content = text.layout.text();
            let length = content.len();
            let range_start = if text.key == start.key {
                start.index.min(length)
            } else {
                0
            };
            let range_end = if text.key == end.key {
                end.index.min(length)
            } else {
                length
            };
            let range = floor_char_boundary(&content, range_start)
                ..floor_char_boundary(&content, range_end.max(range_start));
            self.pieces.push(SelectedPiece {
                key: text.key,
                range: range.clone(),
                text: content[range.clone()].to_string(),
            });
            self.resolved.insert(text.key, range);
        }
    }

    pub(crate) fn selected_text(&self, texts: &[SelectableText]) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let bounds: FxHashMap<TextKey, Bounds<Pixels>> = texts
            .iter()
            .map(|text| (text.key, text.layout.bounds()))
            .collect();
        let mut output = String::new();
        let mut previous: Option<Bounds<Pixels>> = None;
        for piece in self.pieces.iter().filter(|piece| !piece.range.is_empty()) {
            let current = bounds.get(&piece.key).copied();
            if !output.is_empty() {
                let separator = match (previous, current) {
                    (Some(previous), Some(current))
                        if current.top() < previous.bottom()
                            && current.bottom() > previous.top() =>
                    {
                        if current.left() - previous.right() >= Pixels(1.) {
                            " "
                        } else {
                            ""
                        }
                    }
                    _ => "\n",
                };
                output.push_str(separator);
            }
            output.push_str(&piece.text);
            previous = current;
        }
        Some(output)
    }

    fn apply_granularity(&mut self, point: TextPoint, texts: &[SelectableText]) {
        let Some((initial_start, initial_end)) = self.initial else {
            self.head = Some(point);
            return;
        };
        let order: FxHashMap<TextKey, usize> = texts
            .iter()
            .enumerate()
            .map(|(position, text)| (text.key, position))
            .collect();
        let Some(text) = texts.iter().find(|text| text.key == point.key) else {
            return;
        };
        let content = text.layout.text();
        let unit = match self.granularity {
            Granularity::Character => point.index..point.index,
            Granularity::Word => word_range(&content, point.index),
            Granularity::Block => 0..content.len(),
        };
        let unit_start = TextPoint {
            key: point.key,
            index: unit.start,
        };
        let unit_end = TextPoint {
            key: point.key,
            index: unit.end,
        };
        if ordered(unit_start, initial_start, &order) {
            self.anchor = Some(initial_end);
            self.head = Some(unit_start);
        } else {
            self.anchor = Some(initial_start);
            self.head = Some(if ordered(initial_end, unit_end, &order) {
                unit_end
            } else {
                initial_end
            });
        }
    }
}

fn text_point_for(text: &SelectableText, position: Point<Pixels>) -> TextPoint {
    let bounds = text.visible_bounds();
    let clamped = point(
        position.x.clamp(bounds.left(), bounds.right()),
        position.y.clamp(
            bounds.top(),
            (bounds.bottom() - Pixels(0.5)).max(bounds.top()),
        ),
    );
    TextPoint {
        key: text.key,
        index: text.layout.selection_index_for_point(clamped),
    }
}

fn distance_to(bounds: Bounds<Pixels>, position: Point<Pixels>) -> (f32, f32) {
    let horizontal = if position.x < bounds.left() {
        bounds.left() - position.x
    } else if position.x > bounds.right() {
        position.x - bounds.right()
    } else {
        Pixels::ZERO
    };
    let vertical = if position.y < bounds.top() {
        bounds.top() - position.y
    } else if position.y > bounds.bottom() {
        position.y - bounds.bottom()
    } else {
        Pixels::ZERO
    };
    (f32::from(vertical), f32::from(horizontal))
}

fn nearest_text_point(texts: &[SelectableText], position: Point<Pixels>) -> Option<TextPoint> {
    texts
        .iter()
        .filter(|text| text.selectable)
        .filter(|text| {
            let bounds = text.visible_bounds();
            bounds.size.width > Pixels::ZERO && bounds.size.height > Pixels::ZERO
        })
        .min_by(|first, second| {
            distance_to(first.visible_bounds(), position)
                .partial_cmp(&distance_to(second.visible_bounds(), position))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|text| text_point_for(text, position))
}

impl Window {
    fn text_under_mouse(&self, position: Point<Pixels>) -> Option<&SelectableText> {
        let topmost_hitbox = self.mouse_hit_test.ids.first().and_then(|id| {
            self.rendered_frame
                .hitboxes
                .iter()
                .rposition(|hitbox| hitbox.id == *id)
        });
        self.rendered_frame
            .selectable_texts
            .iter()
            .rev()
            .filter(|text| text.visible_bounds().contains(&position))
            .find(|text| topmost_hitbox.is_none_or(|topmost| text.hitbox_floor > topmost))
            .filter(|text| text.selectable)
    }

    pub(crate) fn handle_text_selection_event(&mut self, event: &dyn std::any::Any, cx: &mut App) {
        if let Some(event) = event.downcast_ref::<MouseDownEvent>() {
            self.text_selection_mouse_down(event);
        } else if let Some(event) = event.downcast_ref::<MouseMoveEvent>() {
            self.text_selection_mouse_move(event, cx);
        } else if let Some(event) = event.downcast_ref::<MouseUpEvent>()
            && event.button == MouseButton::Left
        {
            self.text_selection.stop_dragging();
        }
        self.text_selection.suppressed = false;
    }

    fn text_selection_mouse_down(&mut self, event: &MouseDownEvent) {
        if event.button != MouseButton::Left {
            return;
        }
        let suppressed = self.text_selection.suppressed;
        let target = if suppressed {
            None
        } else {
            self.text_under_mouse(event.position).map(|text| {
                (
                    text_point_for(text, event.position),
                    text.layout.text(),
                    text.mask,
                )
            })
        };
        if event.modifiers.shift
            && let (Some((point, _, region)), Some(_anchor)) = (&target, self.text_selection.anchor)
        {
            self.text_selection.head = Some(*point);
            self.text_selection.scroll_region = Some(*region);
            self.text_selection.granularity = Granularity::Character;
            self.text_selection.initial = None;
            self.text_selection.dragging = true;
            self.text_selection
                .resolve(&self.rendered_frame.selectable_texts);
            self.refresh();
            return;
        }
        let had_selection = self.text_selection.clear();
        let Some((point, content, region)) = target else {
            if had_selection {
                self.refresh();
            }
            return;
        };
        let (granularity, range) = match event.click_count {
            0 | 1 => (Granularity::Character, point.index..point.index),
            2 => (Granularity::Word, word_range(&content, point.index)),
            _ => (Granularity::Block, 0..content.len()),
        };
        let start = TextPoint {
            key: point.key,
            index: range.start,
        };
        let end = TextPoint {
            key: point.key,
            index: range.end,
        };
        self.text_selection.anchor = Some(start);
        self.text_selection.head = Some(end);
        self.text_selection.granularity = granularity;
        self.text_selection.initial =
            (granularity != Granularity::Character).then_some((start, end));
        self.text_selection.dragging = true;
        self.text_selection.scroll_region = Some(region);
        self.text_selection
            .resolve(&self.rendered_frame.selectable_texts);
        self.refresh();
    }

    fn text_selection_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut App) {
        if !self.text_selection.dragging {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) || cx.has_active_drag() {
            self.text_selection.stop_dragging();
            if cx.has_active_drag() && self.text_selection.clear() {
                self.refresh();
            }
            return;
        }
        self.update_text_selection_head(event.position);
        self.update_text_selection_autoscroll(event.position, cx);
    }

    fn update_text_selection_autoscroll(&mut self, position: Point<Pixels>, cx: &mut App) {
        let needs_autoscroll = self
            .text_selection
            .scroll_region
            .is_some_and(|region| autoscroll_delta(region, position) != Point::default());
        if !needs_autoscroll {
            self.text_selection.autoscrolling = false;
            self.text_selection.autoscroll_task = None;
            return;
        }
        if self.text_selection.autoscrolling {
            return;
        }
        self.text_selection.autoscrolling = true;
        let handle = self.handle;
        self.text_selection.autoscroll_task = Some(cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(AUTOSCROLL_INTERVAL).await;
                let keep_going = handle
                    .update(cx, |_root, window, cx| {
                        window.text_selection_autoscroll_tick(cx)
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }

    fn text_selection_autoscroll_tick(&mut self, cx: &mut App) -> bool {
        let position = self.mouse_position;
        let delta = match self.text_selection.scroll_region {
            Some(region) if self.text_selection.dragging => {
                Some((region, autoscroll_delta(region, position)))
            }
            _ => None,
        };
        let Some((region, delta)) = delta.filter(|(_, delta)| *delta != Point::default()) else {
            self.text_selection.autoscrolling = false;
            return false;
        };
        let inside = point(
            position
                .x
                .clamp(region.left() + Pixels(1.), region.right() - Pixels(1.)),
            position
                .y
                .clamp(region.top() + Pixels(1.), region.bottom() - Pixels(1.)),
        );
        let event = ScrollEvent {
            position: inside,
            delta: ScrollDelta::Pixels(delta),
            modifiers: self.modifiers(),
            touch_phase: TouchPhase::Moved,
            source: ScrollSource::Pointer,
        };
        self.mouse_position = inside;
        cx.propagate_event = true;
        self.default_prevented = false;
        self.dispatch_mouse_event(&event, cx);
        self.mouse_position = position;
        self.mouse_hit_test = self.rendered_frame.hit_test(position);
        self.update_text_selection_head(position);
        true
    }

    pub(crate) fn text_selection_cursor(&self) -> Option<CursorStyle> {
        if self.text_selection.dragging {
            return Some(CursorStyle::IBeam);
        }
        self.text_under_mouse(self.mouse_position)
            .map(|_| CursorStyle::IBeam)
    }

    fn update_text_selection_head(&mut self, position: Point<Pixels>) {
        let Some(point) = nearest_text_point(&self.rendered_frame.selectable_texts, position)
        else {
            return;
        };
        let previous = (self.text_selection.anchor, self.text_selection.head);
        if self.text_selection.granularity == Granularity::Character {
            self.text_selection.head = Some(point);
        } else {
            self.text_selection
                .apply_granularity(point, &self.rendered_frame.selectable_texts);
        }
        if previous != (self.text_selection.anchor, self.text_selection.head) {
            self.text_selection
                .resolve(&self.rendered_frame.selectable_texts);
            self.refresh();
        }
    }

    pub(crate) fn register_selectable_text(
        &mut self,
        layout: &TextLayout,
        hitbox_floor: usize,
        selectable: bool,
    ) -> Option<Range<usize>> {
        let path = self.element_id_stack.path_hash();
        let ordinal = self
            .next_frame
            .selectable_text_ordinals
            .entry(path)
            .or_default();
        let key = TextKey {
            path,
            ordinal: *ordinal,
        };
        *ordinal += 1;
        let mask = self.content_mask().bounds;
        self.next_frame.selectable_texts.push(SelectableText {
            key,
            layout: layout.clone(),
            mask,
            hitbox_floor,
            selectable,
        });
        if selectable {
            self.text_selection.range_for(key)
        } else {
            None
        }
    }

    pub(crate) fn resolve_text_selection(&mut self) {
        if self.text_selection.anchor.is_some() {
            self.text_selection
                .resolve(&self.rendered_frame.selectable_texts);
        }
    }

    pub(crate) fn text_selection_newline_extension(font_size: Pixels) -> Pixels {
        font_size * NEWLINE_EXTENSION_EM
    }

    /// Stops a text selection from starting on the mouse down event currently being dispatched,
    /// like `preventDefault()` on a browser `mousedown`. Elements that start their own drags
    /// (sortable lists, sliders, canvases) call this from their mouse down listener.
    pub fn prevent_text_selection(&mut self) {
        self.text_selection.suppressed = true;
    }

    /// The text currently selected with the mouse, joined like a browser's `getSelection()`:
    /// texts on the same row are joined with a space when there is a gap between them, texts on
    /// different rows with a newline.
    pub fn selected_text(&self) -> Option<String> {
        self.text_selection
            .selected_text(&self.rendered_frame.selectable_texts)
    }

    /// Whether any text is currently selected with the mouse.
    pub fn has_text_selection(&self) -> bool {
        !self.text_selection.is_empty()
    }

    /// Whether the position is on the glyphs of selectable, unoccluded text, rather than on the
    /// blank part of a text element's box (the space after a short line or below the last one).
    pub fn is_over_text(&self, position: Point<Pixels>) -> bool {
        self.text_under_mouse(position)
            .is_some_and(|text| text.layout.index_for_position(position).is_ok())
    }

    /// Removes the current mouse text selection.
    pub fn clear_text_selection(&mut self) {
        if self.text_selection.clear() {
            self.refresh();
        }
    }

    /// Sets the background painted behind selected text, like CSS `::selection`.
    pub fn set_text_selection_color(&mut self, color: Hsla) {
        if self.text_selection.color != color {
            self.text_selection.color = color;
            self.refresh();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{autoscroll_delta, word_range};
    use crate::{
        AppContext as _, Context, CursorStyle, InputEvent, IntoElement, MouseMoveEvent,
        ParentElement, Render, Styled, TestAppContext, Window, bounds, div, point, px, size,
    };

    struct Labels;

    impl Render for Labels {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(div().h(px(20.)).child("Selectable text"))
                .child(div().h(px(20.)).cursor_pointer().child("Clickable text"))
                .child(div().h(px(20.)).user_select_none().child("Locked text"))
        }
    }

    fn cursor_at(position: crate::Point<crate::Pixels>, cx: &mut TestAppContext) -> CursorStyle {
        let window = cx.add_window(|_, _| Labels);
        let observer = cx.clone();
        cx.update_window(window.into(), |_, window, cx| {
            window.hovered.set(true);
            window.active.set(true);
            window.draw(cx).clear(cx);
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .expect("window updates");
        observer.test_platform().cursor_style()
    }

    #[crate::test]
    fn selectable_text_shows_an_ibeam_unless_an_element_sets_a_cursor(cx: &mut TestAppContext) {
        assert_eq!(cursor_at(point(px(2.), px(10.)), cx), CursorStyle::IBeam);
        assert_eq!(
            cursor_at(point(px(2.), px(30.)), cx),
            CursorStyle::PointingHand
        );
        assert_eq!(cursor_at(point(px(2.), px(50.)), cx), CursorStyle::Arrow);
        assert_eq!(cursor_at(point(px(2.), px(70.)), cx), CursorStyle::Arrow);
    }

    fn over_text_at(position: crate::Point<crate::Pixels>, cx: &mut TestAppContext) -> bool {
        let window = cx.add_window(|_, _| Labels);
        cx.update_window(window.into(), |_, window, cx| {
            window.hovered.set(true);
            window.active.set(true);
            window.draw(cx).clear(cx);
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.is_over_text(position)
        })
        .expect("window updates")
    }

    #[crate::test]
    fn only_glyphs_of_selectable_text_count_as_text(cx: &mut TestAppContext) {
        assert!(over_text_at(point(px(2.), px(10.)), cx));
        assert!(!over_text_at(point(px(600.), px(10.)), cx));
        assert!(!over_text_at(point(px(2.), px(50.)), cx));
        assert!(!over_text_at(point(px(2.), px(70.)), cx));
    }

    #[test]
    fn autoscroll_only_moves_outside_the_region() {
        let region = bounds(point(px(0.), px(100.)), size(px(200.), px(100.)));
        assert_eq!(
            autoscroll_delta(region, point(px(50.), px(150.))),
            point(px(0.), px(0.))
        );
        assert_eq!(
            autoscroll_delta(region, point(px(50.), px(240.))),
            point(px(0.), px(-20.))
        );
        assert_eq!(
            autoscroll_delta(region, point(px(50.), px(99.))),
            point(px(0.), px(2.))
        );
        assert_eq!(
            autoscroll_delta(region, point(px(50.), px(0.))),
            point(px(0.), px(40.))
        );
    }

    #[test]
    fn word_ranges_follow_character_classes() {
        let text = "hello, world_1  done\nnext";
        assert_eq!(word_range(text, 2), 0..5);
        assert_eq!(word_range(text, 5), 5..6);
        assert_eq!(word_range(text, 9), 7..14);
        assert_eq!(word_range(text, 14), 14..16);
        assert_eq!(word_range(text, 22), 21..25);
        assert_eq!(word_range(text, 20), 20..20);
    }
}
