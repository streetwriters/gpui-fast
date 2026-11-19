//! Tests of which scroll container a wheel or trackpad scroll moves. See
//! [`crate::fast::scroll_chain`].
//!
//! Each scenario of `scroll_chain/fixture.json`, which says what Chrome
//! does with it, has a test here: it builds the scenario's nested scroll
//! boxes, finds the one under the pointer in the accessibility tree, sends
//! the scenario's wheel ticks and trackpad phases, and checks that exactly
//! the boxes the fixture names moved, each the way it says. The same
//! scenarios run again with a `list` or `uniform_list` for the inner box.

use std::{collections::BTreeMap, rc::Rc, time::Duration};

use serde::Deserialize;

use crate::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ListAlignment, ListOffset,
    ListState, Modifiers, ParentElement as _, Pixels, Point, Render, Role, ScrollDelta,
    ScrollEvent, ScrollHandle, ScrollSource, SharedString, StatefulInteractiveElement as _,
    Styled as _, TestAppContext, TouchPhase, UniformListScrollHandle, VisualTestContext, Window,
    div, list, point, px, uniform_list,
};

const FIXTURE: &str = include_str!("scroll_chain/fixture.json");

/// The height of a row when a box is a list.
const ROW: f32 = 30.;

#[derive(Deserialize)]
struct Fixture {
    scenarios: Vec<Scenario>,
}

#[derive(Deserialize, Clone)]
struct Scenario {
    name: String,
    layout: Node,
    pointer: String,
    input: Vec<Input>,
    expect: BTreeMap<String, String>,
    why: String,
}

#[derive(Deserialize, Clone)]
struct Node {
    id: String,
    scroll: String,
    size: [f32; 2],
    content: [f32; 2],
    #[serde(default)]
    start: [f32; 2],
    #[serde(default)]
    concurrent: bool,
    /// `restrict_scroll_to_axis`, which browsers have no counterpart of: for
    /// the GPUI-only tests.
    #[serde(default)]
    restrict: bool,
    #[serde(default)]
    child_at: [f32; 2],
    child: Option<Box<Node>>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
struct Input {
    wheel: Option<[f32; 2]>,
    shift: bool,
    trackpad: Option<String>,
    delta: [f32; 2],
    wait: Option<u64>,
    move_pointer: Option<[f32; 2]>,
}

/// What a box is built as.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Div,
    List,
    UniformList,
}

#[derive(Clone)]
enum Handle {
    Div(ScrollHandle),
    List(ListState),
    UniformList(UniformListScrollHandle),
}

impl Handle {
    /// The box's `[scrollLeft, scrollTop]`.
    fn position(&self) -> Point<Pixels> {
        match self {
            Handle::Div(handle) => -handle.offset(),
            Handle::List(state) => {
                let top = state.logical_scroll_top();
                point(px(0.), px(top.item_ix as f32 * ROW) + top.offset_in_item)
            }
            Handle::UniformList(handle) => -handle.0.borrow().base_handle.offset(),
        }
    }
}

fn scenario(name: &str) -> Scenario {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    fixture
        .scenarios
        .into_iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("no scenario {name} in the fixture"))
}

/// Every scenario's name, for the test that checks each has a test.
fn scenario_names() -> Vec<String> {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    fixture.scenarios.into_iter().map(|s| s.name).collect()
}

fn nodes(node: &Node) -> Vec<&Node> {
    let mut nodes = vec![node];
    if let Some(child) = &node.child {
        nodes.extend(self::nodes(child));
    }
    nodes
}

fn make_handle(node: &Node, kind: Kind) -> Handle {
    let [left, top] = node.start;
    match kind {
        Kind::Div => {
            let handle = ScrollHandle::new();
            handle.set_offset(point(px(-left), px(-top)));
            Handle::Div(handle)
        }
        Kind::List => {
            let rows = (node.content[1] / ROW) as usize;
            // A list knows how far it scrolls only from the rows it has
            // measured, so it measures some past its bottom, as apps' do.
            let state = ListState::new(rows, ListAlignment::Top, px(300.));
            state.scroll_to(ListOffset {
                item_ix: (top / ROW) as usize,
                offset_in_item: px(top % ROW),
            });
            Handle::List(state)
        }
        Kind::UniformList => {
            let handle = UniformListScrollHandle::new();
            handle
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(-left), px(-top)));
            Handle::UniformList(handle)
        }
    }
}

/// The scenario's boxes, nested.
struct Nest {
    layout: Node,
    handles: Rc<BTreeMap<String, Handle>>,
}

impl Render for Nest {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(build(&self.layout, &self.handles))
    }
}

fn rows(id: &str, range: std::ops::Range<usize>) -> Vec<AnyElement> {
    range
        .map(|row| {
            div()
                .h(px(ROW))
                .w_full()
                .child(format!("{id} {row}"))
                .into_any_element()
        })
        .collect()
}

fn build(node: &Node, handles: &BTreeMap<String, Handle>) -> AnyElement {
    let [width, height] = node.size;
    match &handles[&node.id] {
        Handle::List(state) => {
            let id = node.id.clone();
            return list(state.clone(), move |row, _, _| {
                rows(&id, row..row + 1).pop().unwrap()
            })
            .w(px(width))
            .h(px(height))
            .flex_none()
            .into_any_element();
        }
        Handle::UniformList(handle) => {
            let id = node.id.clone();
            let count = (node.content[1] / ROW) as usize;
            return uniform_list(
                SharedString::from(node.id.clone()),
                count,
                move |range, _, _| rows(&id, range),
            )
            .track_scroll(handle)
            .w(px(width))
            .h(px(height))
            .flex_none()
            .into_any_element();
        }
        Handle::Div(_) => {}
    }
    let Handle::Div(handle) = &handles[&node.id] else {
        unreachable!()
    };
    let content = div()
        .relative()
        .flex_none()
        .w(px(node.content[0]))
        .h(px(node.content[1]))
        .children(node.child.as_ref().map(|child| {
            div()
                .absolute()
                .left(px(node.child_at[0]))
                .top(px(node.child_at[1]))
                .child(build(child, handles))
        }));
    let scroller = div()
        .id(SharedString::from(node.id.clone()))
        .aria_label(node.id.clone())
        .flex()
        .flex_none()
        .w(px(width))
        .h(px(height))
        .track_scroll(handle);
    let mut scroller = match node.scroll.as_str() {
        "x" => scroller.overflow_x_scroll(),
        "y" => scroller.overflow_y_scroll(),
        "xy" => scroller.overflow_scroll(),
        other => panic!("a box scrolls on x, y or xy, not {other:?}"),
    };
    if node.concurrent {
        scroller.style().allow_concurrent_scroll = Some(true);
    }
    if node.restrict {
        scroller = scroller.restrict_scroll_to_axis();
    }
    scroller.child(content).into_any_element()
}

/// A box's movement as words: `down`, `up`, `left`, `right`, sorted.
fn movement(before: Point<Pixels>, after: Point<Pixels>) -> String {
    let mut words = Vec::new();
    if after.y > before.y + px(0.01) {
        words.push("down");
    } else if after.y < before.y - px(0.01) {
        words.push("up");
    }
    if after.x > before.x + px(0.01) {
        words.push("right");
    } else if after.x < before.x - px(0.01) {
        words.push("left");
    }
    words.sort();
    words.join(" ")
}

fn sorted(words: &str) -> String {
    let mut words: Vec<_> = words.split_whitespace().collect();
    words.sort();
    words.join(" ")
}

/// Opens the scenario's boxes, with `kinds` saying which are lists.
fn open<'a>(
    cx: &'a mut TestAppContext,
    scenario: &Scenario,
    kinds: &[(&str, Kind)],
) -> (&'a mut VisualTestContext, Rc<BTreeMap<String, Handle>>) {
    let handles: BTreeMap<String, Handle> = nodes(&scenario.layout)
        .into_iter()
        .map(|node| {
            let kind = kinds
                .iter()
                .find(|(id, _)| *id == node.id)
                .map_or(Kind::Div, |(_, kind)| *kind);
            (node.id.clone(), make_handle(node, kind))
        })
        .collect();
    let handles = Rc::new(handles);
    let layout = scenario.layout.clone();
    let (_, cx) = cx.add_window_view({
        let handles = handles.clone();
        move |_, _| Nest { layout, handles }
    });
    crate::fast::ui_test::settle(cx);
    (cx, handles)
}

/// Where the pointer goes over the box named `id`: its middle, or for a
/// list the middle of the first row it shows whole.
fn pointer_over(
    cx: &mut VisualTestContext,
    handles: &BTreeMap<String, Handle>,
    id: &str,
) -> Point<Pixels> {
    let bounds = match &handles[id] {
        Handle::Div(_) => cx.get_by_role(Role::ScrollView, id).bounds(),
        Handle::List(_) | Handle::UniformList(_) => {
            let row = (handles[id].position().y.as_f32() / ROW).floor() as usize + 1;
            cx.get_by_label(&format!("{id} {row}")).bounds()
        }
    };
    bounds
        .unwrap_or_else(|| panic!("{id} has bounds in the accessibility tree"))
        .center()
}

fn event(input: &Input, position: Point<Pixels>) -> ScrollEvent {
    // The fixture's deltas are the way content scrolls, as scrollTop grows;
    // GPUI's are the way it moves.
    if let Some([x, y]) = input.wheel {
        return ScrollEvent {
            position,
            delta: ScrollDelta::Lines(point(-x, -y)),
            modifiers: Modifiers {
                shift: input.shift,
                ..Default::default()
            },
            touch_phase: TouchPhase::Moved,
            source: ScrollSource::Pointer,
        };
    }
    let phase = match input.trackpad.as_deref() {
        Some("start") => TouchPhase::Started,
        Some("move") | Some("momentum") => TouchPhase::Moved,
        Some("end") => TouchPhase::Ended,
        other => panic!("not an input the fixture has: {other:?}"),
    };
    let [x, y] = input.delta;
    ScrollEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(-x), px(-y))),
        modifiers: Modifiers::default(),
        touch_phase: phase,
        source: ScrollSource::Pointer,
    }
}

/// Plays the scenario's input and returns how each box moved.
fn play(
    cx: &mut VisualTestContext,
    scenario: &Scenario,
    handles: &BTreeMap<String, Handle>,
) -> BTreeMap<String, String> {
    let mut pointer = pointer_over(cx, handles, &scenario.pointer);
    let before: BTreeMap<_, _> = handles
        .iter()
        .map(|(id, handle)| (id.clone(), handle.position()))
        .collect();
    for input in &scenario.input {
        if let Some(wait) = input.wait {
            cx.executor().advance_clock(Duration::from_millis(wait));
            continue;
        }
        if let Some([x, y]) = input.move_pointer {
            pointer += point(px(x), px(y));
            continue;
        }
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.simulate_event(event(input, pointer));
        crate::fast::ui_test::settle(cx);
    }
    handles
        .iter()
        .map(|(id, handle)| (id.clone(), movement(before[id], handle.position())))
        .collect()
}

/// Runs the scenario named `name` with `kinds` saying which boxes are
/// lists, and checks that the boxes moved as the fixture expects.
fn check(cx: &mut TestAppContext, name: &str, kinds: &[(&str, Kind)]) {
    check_scenario(cx, scenario(name), kinds);
}

fn check_scenario(cx: &mut TestAppContext, scenario: Scenario, kinds: &[(&str, Kind)]) {
    let (cx, handles) = open(cx, &scenario, kinds);
    let moved = play(cx, &scenario, &handles);
    assert_moved(&scenario, moved, &handles);
}

fn assert_moved(
    scenario: &Scenario,
    moved: BTreeMap<String, String>,
    handles: &BTreeMap<String, Handle>,
) {
    let name = &scenario.name;
    let expected: BTreeMap<String, String> = handles
        .keys()
        .map(|id| {
            let words = scenario.expect.get(id).map_or(String::new(), |w| sorted(w));
            (id.clone(), words)
        })
        .collect();
    assert_eq!(
        moved, expected,
        "{name}: which boxes moved (\"\" = did not move)\nexpected because {}",
        scenario.why
    );
}

macro_rules! scenarios {
    ($($name:ident),* $(,)?) => {
        const TESTED: &[&str] = &[$(stringify!($name)),*];
        $(
            #[crate::test]
            fn $name(cx: &mut TestAppContext) {
                check(cx, stringify!($name), &[]);
            }
        )*
    };
}

scenarios!(
    vertical_in_vertical_wheel,
    vertical_in_vertical_trackpad,
    horizontal_in_vertical_wheel,
    horizontal_in_vertical_trackpad_vertical,
    horizontal_in_vertical_trackpad_horizontal,
    vertical_in_horizontal_wheel,
    vertical_in_horizontal_trackpad_horizontal,
    both_axes_in_vertical_wheel,
    both_axes_in_vertical_trackpad_diagonal,
    inner_at_bottom_wheel_chains,
    inner_at_top_wheel_up_chains,
    inner_at_bottom_trackpad_chains,
    inner_reaches_bottom_mid_gesture_trackpad,
    inner_reaches_bottom_mid_burst_wheel,
    burst_within_timeout_stays_latched,
    new_burst_after_timeout_chains,
    new_trackpad_gesture_at_edge_chains,
    momentum_stays_latched,
    shift_wheel_over_horizontal,
    shift_wheel_over_vertical_only,
    non_overflowing_inner,
    three_levels_vertical,
    three_levels_inner_at_bottom_skips_horizontal,
    three_levels_horizontal_trackpad,
    pointer_moved_past_slop_relatches,
    pointer_moved_within_slop_stays,
);

/// Every scenario in the fixture has a test above.
#[test]
fn every_scenario_has_a_test() {
    for name in scenario_names() {
        assert!(
            TESTED.contains(&name.as_str()),
            "no test for scenario {name}"
        );
    }
}

/// The scenarios with a vertical inner box in a vertical page, the inner
/// box a `list` and then a `uniform_list`: they chain and latch as a `div`.
#[crate::test]
fn lists_chain_and_latch_as_a_div_does(cx: &mut TestAppContext) {
    for kind in [Kind::List, Kind::UniformList] {
        for name in [
            "vertical_in_vertical_wheel",
            "vertical_in_vertical_trackpad",
            "inner_at_bottom_wheel_chains",
            "inner_at_top_wheel_up_chains",
            "inner_at_bottom_trackpad_chains",
            "inner_reaches_bottom_mid_gesture_trackpad",
            "inner_reaches_bottom_mid_burst_wheel",
            "new_burst_after_timeout_chains",
            "momentum_stays_latched",
            "shift_wheel_over_vertical_only",
        ] {
            // Each check opens its own window, with its own latch.
            check(cx, name, &[("inner", kind)]);
        }
    }
}

// What follows has no browser counterpart: GPUI's opt-ins, the way other
// platforms send shift+wheel, and lists around a horizontal box.

/// A scenario written here rather than in the fixture.
fn gpui_only(scenario: serde_json::Value) -> Scenario {
    serde_json::from_value(scenario).expect("a scenario")
}

/// A page with a box inside it that scrolls both ways, scrolled to the
/// middle of both, with `options` (`concurrent`, `restrict`) set on it.
fn both_axes_box(options: serde_json::Value) -> serde_json::Value {
    let mut inner = serde_json::json!({
        "id": "inner", "scroll": "xy", "size": [300, 150], "content": [900, 450], "start": [300, 150]
    });
    inner
        .as_object_mut()
        .unwrap()
        .extend(options.as_object().unwrap().clone());
    serde_json::json!({
        "id": "page", "scroll": "y", "size": [300, 300], "content": [300, 900],
        "child_at": [0, 100], "child": inner
    })
}

/// Windows and Linux send shift+wheel already turned horizontal: it scrolls
/// the horizontal box under the pointer all the same.
#[crate::test]
fn shift_wheel_sent_horizontal_scrolls_a_horizontal_box(cx: &mut TestAppContext) {
    let mut scenario = scenario("shift_wheel_over_horizontal");
    scenario.input = vec![Input {
        wheel: Some([3., 0.]),
        shift: true,
        ..Input::default()
    }];
    check_scenario(cx, scenario, &[]);
}

/// Without `allow_concurrent_scroll`, a box that scrolls both ways moves
/// along the larger axis of each diagonal delta only; with it, along both.
#[crate::test]
fn allow_concurrent_scroll_lets_a_box_move_diagonally(cx: &mut TestAppContext) {
    let input = serde_json::json!([
        {"trackpad": "start", "delta": [0, 0]},
        {"trackpad": "move", "delta": [20, 10]},
        {"trackpad": "move", "delta": [20, 10]},
        {"trackpad": "end", "delta": [0, 0]}
    ]);
    check_scenario(
        cx,
        gpui_only(serde_json::json!({
            "name": "a diagonal swipe over a box without allow_concurrent_scroll",
            "layout": both_axes_box(serde_json::json!({})),
            "pointer": "inner", "input": input,
            "expect": {"inner": "right"},
            "why": "the box takes the gesture and moves along its larger axis only"
        })),
        &[],
    );
    check_scenario(
        cx,
        gpui_only(serde_json::json!({
            "name": "a diagonal swipe over a box with allow_concurrent_scroll",
            "layout": both_axes_box(serde_json::json!({"concurrent": true})),
            "pointer": "inner", "input": input,
            "expect": {"inner": "down right"},
            "why": "allow_concurrent_scroll moves it along both axes, as a browser does"
        })),
        &[],
    );
}

/// `restrict_scroll_to_axis` keeps a trackpad gesture on the axis it began
/// along, within the box the gesture went to; it never sends the other
/// axis to another box.
#[crate::test]
fn restrict_scroll_to_axis_keeps_a_gesture_on_its_axis(cx: &mut TestAppContext) {
    // A vertical start, then deltas a little diagonal but still mostly
    // vertical, so the lock holds however long the test takes between them.
    let input = serde_json::json!([
        {"trackpad": "start", "delta": [0, 0]},
        {"trackpad": "move", "delta": [0, 20]},
        {"trackpad": "move", "delta": [2, 3]},
        {"trackpad": "move", "delta": [2, 3]},
        {"trackpad": "end", "delta": [0, 0]}
    ]);
    check_scenario(
        cx,
        gpui_only(serde_json::json!({
            "name": "a vertical swipe drifting sideways over a box with restrict_scroll_to_axis",
            "layout": both_axes_box(serde_json::json!({"concurrent": true, "restrict": true})),
            "pointer": "inner", "input": input,
            "expect": {"inner": "down"},
            "why": "the gesture began vertical, so the box drops its horizontal drift, and the page gets none of it"
        })),
        &[],
    );
    check_scenario(
        cx,
        gpui_only(serde_json::json!({
            "name": "the same swipe over a box without it",
            "layout": both_axes_box(serde_json::json!({"concurrent": true})),
            "pointer": "inner", "input": input,
            "expect": {"inner": "down right"},
            "why": "without the lock the drift moves the box sideways too"
        })),
        &[],
    );
}

/// A `list` or `uniform_list` page whose second row is a horizontal strip.
struct ListPage {
    handles: Rc<BTreeMap<String, Handle>>,
}

fn list_page_row(row: usize, strip: &ScrollHandle) -> AnyElement {
    if row != 1 {
        return rows("page", row..row + 1).pop().unwrap();
    }
    div()
        .id("strip")
        .aria_label("strip")
        .flex()
        .flex_none()
        .w(px(300.))
        .h(px(ROW))
        .overflow_x_scroll()
        .track_scroll(strip)
        .child(div().flex_none().w(px(900.)).h(px(ROW)))
        .into_any_element()
}

impl Render for ListPage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let Handle::Div(strip) = self.handles["strip"].clone() else {
            unreachable!()
        };
        let page = match self.handles["page"].clone() {
            Handle::List(state) => list(state, move |row, _, _| list_page_row(row, &strip))
                .w(px(300.))
                .h(px(150.))
                .into_any_element(),
            Handle::UniformList(handle) => uniform_list("page", 30, move |range, _, _| {
                range
                    .map(|row| list_page_row(row, &strip))
                    .collect::<Vec<_>>()
            })
            .track_scroll(&handle)
            .w(px(300.))
            .h(px(150.))
            .into_any_element(),
            Handle::Div(_) => unreachable!(),
        };
        div().size_full().child(page)
    }
}

/// Over a horizontal strip inside a `list` or `uniform_list`, a vertical
/// wheel scrolls the list only and a horizontal swipe the strip only.
#[crate::test]
fn a_horizontal_box_in_a_list_takes_only_horizontal_scrolls(cx: &mut TestAppContext) {
    let page =
        serde_json::json!({"id": "page", "scroll": "y", "size": [300, 150], "content": [300, 900]});
    let wheel = gpui_only(serde_json::json!({
        "name": "a vertical wheel over a strip in a list",
        "layout": page, "pointer": "strip",
        "input": [{"wheel": [0, 1]}],
        "expect": {"page": "down"},
        "why": "the strip cannot scroll vertically, so the list takes the wheel"
    }));
    let swipe = gpui_only(serde_json::json!({
        "name": "a horizontal swipe over a strip in a list",
        "layout": page, "pointer": "strip",
        "input": [
            {"trackpad": "start", "delta": [0, 0]},
            {"trackpad": "move", "delta": [20, 1]},
            {"trackpad": "move", "delta": [20, 0]},
            {"trackpad": "end", "delta": [0, 0]}
        ],
        "expect": {"strip": "right"},
        "why": "the swipe is horizontal, which only the strip scrolls"
    }));
    for kind in [Kind::List, Kind::UniformList] {
        for scenario in [&wheel, &swipe] {
            let page = make_handle(&scenario.layout, kind);
            let handles: Rc<BTreeMap<String, Handle>> = Rc::new(BTreeMap::from([
                ("page".to_string(), page),
                ("strip".to_string(), Handle::Div(ScrollHandle::new())),
            ]));
            let (_, cx) = cx.add_window_view({
                let handles = handles.clone();
                move |_, _| ListPage { handles }
            });
            crate::fast::ui_test::settle(cx);
            let moved = play(cx, scenario, &handles);
            assert_moved(scenario, moved, &handles);
        }
    }
}
