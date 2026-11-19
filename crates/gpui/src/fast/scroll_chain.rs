//! Which scroll container a wheel or trackpad scroll moves: one, chosen the
//! way Chrome chooses it, and kept for the whole gesture.
//!
//! Each scroll container (a `div` with `overflow_*_scroll`, a `uniform_list`,
//! a `list`) listens to every wheel event. In the capture phase, outermost
//! first, each one *offers* itself: whether it is under the pointer, which
//! axes it scrolls, and how far it can still move. In the bubble phase,
//! innermost first, the first listener *routes* the event, and only the
//! container it was routed to applies it. Nothing stops propagation, so
//! other wheel listeners (a scrollbar showing itself) still hear it.
//!
//! - **Axis.** The event's dominant axis (vertical on a tie) picks the
//!   container: the innermost under the pointer that scrolls on that axis and
//!   can still move in the delta's direction on it. A container applies only
//!   deltas on the axes it scrolls: a vertical wheel over a box that scrolls
//!   only horizontally does not move it, and goes to the box around it.
//!   Shift turns a vertical wheel tick into a horizontal one, as browsers do
//!   (the platforms other than macOS already send it so).
//! - **Chaining.** No other container applies the event, on any axis: a
//!   horizontal box inside a vertical page never scrolls diagonally, and a
//!   page never scrolls with the box under the pointer.
//! - **Latching.** The container the first event of a *sequence* went to
//!   takes every event of the sequence, even once it reaches its edge; a new
//!   sequence that starts at the edge chains to the container around it. A
//!   trackpad gesture is a sequence from its `Started` phase through its
//!   momentum; wheel ticks are one while they come less than
//!   [`WHEEL_LATCHING_TIMEOUT`] apart, within [`WHEEL_LATCHING_SLOP`] of
//!   where the first one was, with the same modifiers. When the first
//!   events of a sequence move nothing, each later one is routed afresh.
//! - **Momentum** after a gesture's `Ended` stays with its container when it
//!   comes within [`MOMENTUM_WAIT`].
//!
//! The constants and rules are Chromium's:
//! `content/browser/renderer_host/input/mouse_wheel_phase_handler.{h,cc}`
//! (the sequence) and `cc/input/input_handler.cc`, `FindNodeToLatch` and
//! `ScrollLatchedScroller` (the latched scroller). See `docs/scrolling.md`.

use std::rc::Rc;
use std::time::Duration;

use scheduler::Instant;
use sum_tree::Bias;

use crate::{
    App, Axis, Count, DispatchPhase, EntityId, Hitbox, HitboxId, Interactivity, IsZero,
    ListItemSummary, ListOffset, ListState, Modifiers, Overflow, Pixels, PlatformInput, Point,
    ScrollDelta, ScrollEvent, Style, TouchPhase, Window, point, px,
};

/// How long after a wheel tick the next one still belongs to its sequence:
/// Chromium's `kDefaultMouseWheelLatchingTransactionTimeout`.
pub(crate) const WHEEL_LATCHING_TIMEOUT: Duration = Duration::from_millis(500);

/// How far from the first tick of a sequence the pointer may be for a tick
/// to still belong to it: Chromium's `kWheelLatchingSlopRegion`.
pub(crate) const WHEEL_LATCHING_SLOP: f32 = 10.;

/// How long after a gesture's `Ended` its momentum may begin and still go to
/// its container: Chromium's `max_time_between_phase_ended_and_momentum_phase_began_`.
pub(crate) const MOMENTUM_WAIT: Duration = Duration::from_millis(100);

/// Room on an axis less than this is none, so an offset a rounding error
/// short of the edge does not keep a sequence from chaining.
const ROOM_EPSILON: f32 = 0.5;

/// A window's scroll sequence and the routing of the event being dispatched.
#[derive(Default)]
pub(crate) struct ScrollChain {
    sequence: Option<Sequence>,
    event: Option<Route>,
}

struct Sequence {
    /// Began with `TouchPhase::Started`: a trackpad or touch gesture, which
    /// ends with its phases rather than a timeout.
    phased: bool,
    precise: bool,
    origin: Point<Pixels>,
    modifiers: Modifiers,
    last: Instant,
    /// The gesture's `Ended` or `Cancelled` came; only momentum follows.
    ended: bool,
    /// The container the sequence went to, by [`key`].
    latched: Option<usize>,
}

impl Sequence {
    fn new(event: &ScrollEvent, now: Instant) -> Self {
        Self {
            phased: event.touch_phase == TouchPhase::Started,
            precise: event.delta.precise(),
            origin: event.position,
            modifiers: event.modifiers,
            last: now,
            ended: false,
            latched: None,
        }
    }

    fn continues_with(&self, event: &ScrollEvent, now: Instant) -> bool {
        if event.touch_phase == TouchPhase::Started {
            return false;
        }
        let gap = now.saturating_duration_since(self.last);
        if self.ended {
            return event.touch_phase == TouchPhase::Moved && gap <= MOMENTUM_WAIT;
        }
        if self.phased {
            // A wheel tick during a gesture begins a sequence of its own.
            return event.delta.precise();
        }
        let moved = event.position - self.origin;
        gap < WHEEL_LATCHING_TIMEOUT
            && f32::from(moved.x).hypot(f32::from(moved.y)) <= WHEEL_LATCHING_SLOP
            && event.modifiers == self.modifiers
            && event.delta.precise() == self.precise
    }
}

/// The event being dispatched: its delta as routed, the containers that
/// offered to take it, and where it went.
struct Route {
    delta: ScrollDelta,
    offers: Vec<Offer>,
    winner: Option<Option<usize>>,
}

struct Offer {
    key: usize,
    under_pointer: bool,
    scrolls: Point<bool>,
    /// The content's offset, from `-max` (scrolled to the end) to zero.
    offset: Point<Pixels>,
    max: Point<Pixels>,
}

impl Offer {
    /// Whether a delta of sign `positive` on `axis` moves the content.
    fn can_move(&self, axis: Axis, positive: bool) -> bool {
        let (scrolls, offset, max) = match axis {
            Axis::Horizontal => (self.scrolls.x, self.offset.x, self.max.x),
            Axis::Vertical => (self.scrolls.y, self.offset.y, self.max.y),
        };
        let room = if positive {
            -f32::from(offset)
        } else {
            f32::from(offset + max)
        };
        scrolls && room > ROOM_EPSILON
    }
}

/// A container's identity across frames: its offset's or state's address.
fn key<T>(state: &Rc<T>) -> usize {
    Rc::as_ptr(state) as *const () as usize
}

/// Notes `event`, if it is a wheel event, as dispatch begins: whether it
/// continues the sequence or begins one, and its delta as routed.
pub(crate) fn begin_event(window: &mut Window, cx: &App, event: &PlatformInput) {
    let PlatformInput::Scroll(event) = event else {
        return;
    };
    let now = cx.background_executor().now();
    let chain = &mut window.fast_layers.scroll_chain;
    match &mut chain.sequence {
        Some(sequence) if sequence.continues_with(event, now) => {
            sequence.last = now;
            sequence.ended = false;
        }
        sequence => *sequence = Some(Sequence::new(event, now)),
    }
    if matches!(event.touch_phase, TouchPhase::Ended | TouchPhase::Cancelled)
        && let Some(sequence) = &mut chain.sequence
    {
        sequence.ended = true;
    }
    chain.event = Some(Route::new(event));
}

/// Forgets the event once it has been dispatched.
pub(crate) fn end_event(window: &mut Window) {
    window.fast_layers.scroll_chain.event = None;
}

/// Ends the sequence, so the next wheel event begins one: for wheel events
/// the window makes up itself, each meant for one container.
pub(crate) fn new_sequence(window: &mut Window) {
    window.fast_layers.scroll_chain.sequence = None;
}

impl Route {
    fn new(event: &ScrollEvent) -> Self {
        Self {
            delta: routed_delta(event),
            offers: Vec::new(),
            winner: None,
        }
    }
}

/// `event`'s delta with shift turning a vertical wheel tick horizontal.
fn routed_delta(event: &ScrollEvent) -> ScrollDelta {
    match event.delta {
        ScrollDelta::Lines(lines) if event.modifiers.shift && lines.x == 0. => {
            ScrollDelta::Lines(point(lines.y, 0.))
        }
        delta => delta,
    }
}

impl ScrollChain {
    /// The event being dispatched, begun here if [`begin_event`] did not see
    /// it (a listener called outside the window's dispatch).
    fn route(&mut self, event: &ScrollEvent) -> &mut Route {
        self.event.get_or_insert_with(|| Route::new(event))
    }

    fn winner(&mut self, event: &ScrollEvent) -> Option<usize> {
        let route = self.event.get_or_insert_with(|| Route::new(event));
        if let Some(winner) = route.winner {
            return winner;
        }
        let sequence = self.sequence.as_mut();
        let latched = sequence
            .as_ref()
            .and_then(|sequence| sequence.latched)
            .filter(|key| route.offers.iter().any(|offer| offer.key == *key));
        let winner = match latched {
            Some(key) => Some(key),
            None => {
                let winner = choose(&route.offers, route.delta);
                if let Some(sequence) = sequence {
                    sequence.latched = winner;
                }
                winner
            }
        };
        route.winner = Some(winner);
        winner
    }
}

/// The innermost container under the pointer that can move along `delta`'s
/// dominant axis, if any.
fn choose(offers: &[Offer], delta: ScrollDelta) -> Option<usize> {
    let delta = match delta {
        ScrollDelta::Pixels(delta) => point(f32::from(delta.x), f32::from(delta.y)),
        ScrollDelta::Lines(delta) => delta,
    };
    if delta.x == 0. && delta.y == 0. {
        return None;
    }
    let (axis, along) = if delta.y.abs() >= delta.x.abs() {
        (Axis::Vertical, delta.y)
    } else {
        (Axis::Horizontal, delta.x)
    };
    offers
        .iter()
        .rev()
        .find(|offer| offer.under_pointer && offer.can_move(axis, along > 0.))
        .map(|offer| offer.key)
}

fn offer(window: &mut Window, event: &ScrollEvent, offer: Offer) {
    window
        .fast_layers
        .scroll_chain
        .route(event)
        .offers
        .push(offer);
}

fn takes(window: &mut Window, event: &ScrollEvent, key: usize) -> Option<ScrollDelta> {
    let chain = &mut window.fast_layers.scroll_chain;
    (chain.winner(event) == Some(key)).then(|| chain.route(event).delta)
}

/// `Interactivity::paint_scroll_listener`: the wheel listener of a `div`
/// (or `uniform_list`) scroll container.
pub(crate) fn paint_div_listener(
    interactivity: &Interactivity,
    hitbox: &Hitbox,
    style: &Style,
    window: &mut Window,
) {
    let Some(scroll_offset) = interactivity.scroll_offset.clone() else {
        return;
    };
    let ongoing_scroll = interactivity.ongoing_scroll.clone();
    let scrolls = point(
        style.overflow.x == Overflow::Scroll,
        style.overflow.y == Overflow::Scroll,
    );
    let allow_concurrent_scroll = style.allow_concurrent_scroll;
    let restrict_scroll_to_axis = style.restrict_scroll_to_axis;
    let line_height = window.line_height();
    let max = crate::fast::layers::wheel::scroll_max(interactivity, hitbox.bounds, style, window);
    let hitbox = hitbox.clone();
    let current_view = window.current_view();
    let subtrees = crate::fast::retained::enclosing_retained_subtrees(window);
    let fast_container = crate::fast::layers::invalidate::painted_container(window, interactivity);
    let key = key(&scroll_offset);
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            let under_pointer = hitbox.should_handle_scroll(window);
            let offset = *scroll_offset.borrow();
            offer(
                window,
                event,
                Offer {
                    key,
                    under_pointer,
                    scrolls,
                    offset,
                    max,
                },
            );
            return;
        }
        let Some(delta) = takes(window, event, key) else {
            return;
        };
        let mut scroll_offset = scroll_offset.borrow_mut();
        let old_scroll_offset = *scroll_offset;
        let mut delta = delta.pixel_delta(line_height);
        if restrict_scroll_to_axis
            && event.delta.precise()
            && let Some(ongoing_scroll) = &ongoing_scroll
        {
            ongoing_scroll
                .borrow_mut()
                .filter(&mut delta, event.touch_phase);
        }
        let mut delta_x = if scrolls.x { delta.x } else { Pixels::ZERO };
        let mut delta_y = if scrolls.y { delta.y } else { Pixels::ZERO };
        if !allow_concurrent_scroll && !delta_x.is_zero() && !delta_y.is_zero() {
            if delta_x.abs() > delta_y.abs() {
                delta_y = Pixels::ZERO;
            } else {
                delta_x = Pixels::ZERO;
            }
        }
        if !crate::fast::layers::wheel::moves(old_scroll_offset, point(delta_x, delta_y), max) {
            return;
        }
        scroll_offset.y += delta_y;
        scroll_offset.x += delta_x;
        if *scroll_offset != old_scroll_offset {
            crate::fast::retained::invalidate_retained_subtrees(window, &subtrees);
            crate::fast::layers::invalidate::note_scrolled(window, &fast_container);
            cx.notify(current_view);
        }
    });
}

/// The wheel listener of a `list`, which scrolls vertically: `list` is its
/// state, its scroll top and height as laid out, its hitbox and its view.
pub(crate) fn paint_list_listener(
    list: (&ListState, ListOffset, Pixels, HitboxId, EntityId),
    window: &mut Window,
) {
    let (state, scroll_top, height, hitbox_id, current_view) = list;
    let list_state = state.clone();
    let key = key(&state.0);
    let mut accumulated_scroll_delta = ScrollDelta::default();
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            let under_pointer = hitbox_id.should_handle_scroll(window);
            let (up, down) = list_room(&list_state, height);
            offer(
                window,
                event,
                Offer {
                    key,
                    under_pointer,
                    scrolls: point(false, true),
                    offset: point(Pixels::ZERO, -up),
                    max: point(Pixels::ZERO, up + down),
                },
            );
            return;
        }
        let Some(delta) = takes(window, event, key) else {
            return;
        };
        accumulated_scroll_delta = accumulated_scroll_delta.coalesce(delta);
        let pixel_delta = accumulated_scroll_delta.pixel_delta(px(20.));
        list_state
            .0
            .borrow_mut()
            .scroll(&scroll_top, height, pixel_delta, current_view, window, cx)
    });
}

/// How far a list of `height` can still scroll up and down. A list knows
/// only the heights of the rows it has measured: rows not yet measured on a
/// side count as a viewport of room, since it cannot know it is at that end
/// until it has measured them. One just reset scrolls nowhere until it is
/// laid out again, as `StateInner::scroll` drops wheels until then.
fn list_room(state: &ListState, height: Pixels) -> (Pixels, Pixels) {
    let state = state.0.borrow();
    if state.reset {
        return (Pixels::ZERO, Pixels::ZERO);
    }
    let top = state.logical_scroll_top();
    let (above, ..) = state
        .items
        .find::<ListItemSummary, _>((), &Count(top.item_ix), Bias::Right);
    let all = state.items.summary();
    let padding = state.last_padding.unwrap_or_default();
    let max = (all.height + padding.top + padding.bottom - height).max(px(0.));
    let scroll_top = above.height + top.offset_in_item;
    let up = if above.unrendered_count > 0 {
        height
    } else {
        scroll_top.min(max)
    };
    let down = if all.unrendered_count > above.unrendered_count {
        height
    } else {
        (max - scroll_top).max(px(0.))
    };
    (up, down)
}
