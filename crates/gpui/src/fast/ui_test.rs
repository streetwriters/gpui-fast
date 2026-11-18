//! Driving a test window the way a user, or a GUI driver like Cua, does: by
//! what is on screen, not by element ids, debug selectors or pixel offsets.
//!
//! Everything here reads the window's accessibility tree, the one assistive
//! technology reads, and acts through the accessibility actions it sends.
//! Clickable elements are buttons named by their text, plain text is a
//! label named by its text, and text fields are found by their label or
//! placeholder. The first call on a window switches accessibility on for it.
//!
//! ```ignore
//! let (_, cx) = cx.add_window_view(|_, _| Editor::new());
//! cx.get_by_role(Role::Button, "Save").click();
//! cx.assert_shown("Saved");
//! cx.type_into("Search", "hello");
//! cx.get_by_label("Row 30").scroll_into_view();
//! println!("{}", cx.a11y_tree_text());
//! ```
//!
//! See `docs/ui-testing.md` for the whole API.

use std::fmt::Write as _;
use std::time::Duration;

use accesskit::{Action, NodeId, Role};

use crate::{Bounds, Pixels, VisualTestContext, Window, point, px, size};

/// One node of the window's accessibility tree, as the tests here see it.
#[derive(Clone, Debug)]
struct Entry {
    id: NodeId,
    depth: usize,
    /// Index of the parent in [`Snapshot::entries`].
    parent: Option<usize>,
    role: Role,
    name: Option<String>,
    placeholder: Option<String>,
    value: Option<String>,
    description: Option<String>,
    /// Ids of the nodes that label this one.
    labelled_by: Vec<NodeId>,
    focused: bool,
    focusable: bool,
    disabled: bool,
    /// Whether any of it is inside the window and every scroll container
    /// around it.
    shown: bool,
}

impl Entry {
    fn line(&self) -> String {
        let mut line = format!("{:?}", self.role);
        if let Some(name) = &self.name {
            write!(line, " {name:?}").ok();
        }
        if let Some(placeholder) = &self.placeholder {
            write!(line, " placeholder={placeholder:?}").ok();
        }
        if let Some(value) = &self.value {
            write!(line, " value={value:?}").ok();
        }
        if let Some(description) = &self.description {
            write!(line, " description={description:?}").ok();
        }
        if self.focused {
            line.push_str(" [focused]");
        }
        if self.disabled {
            line.push_str(" [disabled]");
        }
        if !self.shown {
            line.push_str(" [off-screen]");
        }
        line
    }

    /// The text it shows or is called by: its name, or for a field its value.
    fn texts(&self) -> impl Iterator<Item = &str> {
        self.name.iter().chain(self.value.iter()).map(String::as_str)
    }
}

/// The window's last accessibility tree in depth-first order.
struct Snapshot {
    entries: Vec<Entry>,
}

impl Snapshot {
    fn take(window: &Window) -> Self {
        let tree = crate::fast::a11y::last_tree(window)
            .expect("the window has drawn no accessibility tree yet");
        let nodes: collections::FxHashMap<NodeId, &accesskit::Node> =
            tree.nodes.iter().map(|(id, node)| (*id, node)).collect();
        let scale = window.scale_factor() as f64;
        let to_bounds = |rect: accesskit::Rect| {
            Bounds::new(
                point(px((rect.x0 / scale) as f32), px((rect.y0 / scale) as f32)),
                size(
                    px(((rect.x1 - rect.x0) / scale) as f32),
                    px(((rect.y1 - rect.y0) / scale) as f32),
                ),
            )
        };
        let viewport = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let mut entries: Vec<Entry> = Vec::new();
        // (id, depth, parent, the area it can show in)
        let mut stack = vec![(crate::window::a11y::ROOT_NODE_ID, 0, None, viewport)];
        while let Some((id, depth, parent, clip)) = stack.pop() {
            let Some(node) = nodes.get(&id) else {
                continue;
            };
            let bounds = node.bounds().map(to_bounds);
            let shown = bounds.is_none_or(|bounds| intersects(&bounds, &clip));
            let inner_clip = match (node.role(), bounds) {
                (Role::ScrollView, Some(bounds)) => clip.intersect(&bounds),
                _ => clip,
            };
            let index = entries.len();
            entries.push(Entry {
                id,
                depth,
                parent,
                role: node.role(),
                name: node.label().map(str::to_string),
                placeholder: node.placeholder().map(str::to_string),
                value: node.value().map(str::to_string),
                description: node.description().map(str::to_string),
                labelled_by: node.labelled_by().to_vec(),
                focused: id == tree.focus && id != crate::window::a11y::ROOT_NODE_ID,
                focusable: window.a11y.focus_ids.contains_key(&id),
                disabled: node.is_disabled(),
                shown: shown && parent.is_none_or(|parent: usize| entries[parent].shown),
            });
            for child in node.children().iter().rev() {
                stack.push((*child, depth + 1, Some(index), inner_clip));
            }
        }
        Self { entries }
    }

    fn dump(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            writeln!(out, "{}{}", "  ".repeat(entry.depth), entry.line()).ok();
        }
        out
    }

    /// The roles and names of the nodes around `entry`, outermost first.
    fn ancestry(&self, entry: &Entry) -> String {
        let mut chain = Vec::new();
        let mut at = entry.parent;
        while let Some(index) = at {
            let parent = &self.entries[index];
            chain.push(match &parent.name {
                Some(name) => format!("{:?} {name:?}", parent.role),
                None => format!("{:?}", parent.role),
            });
            at = parent.parent;
        }
        chain.reverse();
        chain.join(" > ")
    }

    /// The one entry `matches` picks, or a panic saying what there is.
    fn the_one(&self, what: &str, hint: &str, matches: impl Fn(&Entry) -> bool) -> Entry {
        let found: Vec<&Entry> = self.entries.iter().filter(|e| matches(e)).collect();
        match found.as_slice() {
            [one] => (*one).clone(),
            [] => panic!(
                "no {what} in the window.\n{}\nThe window's accessibility tree:\n{}",
                self.like(hint),
                self.dump()
            ),
            many => {
                let mut list = String::new();
                for entry in many {
                    writeln!(list, "  {}   (in {})", entry.line(), self.ancestry(entry)).ok();
                }
                panic!(
                    "{} matches for {what}, expected one:\n{list}\
                     Use a longer name, or the exact one.\n\
                     The window's accessibility tree:\n{}",
                    many.len(),
                    self.dump()
                )
            }
        }
    }

    /// The named nodes whose name looks like `hint`, for a failure message.
    fn like(&self, hint: &str) -> String {
        let hint = hint.to_lowercase();
        let words: Vec<&str> = hint.split_whitespace().collect();
        let similar: Vec<String> = self
            .entries
            .iter()
            .filter(|entry| {
                entry.texts().chain(entry.placeholder.as_deref()).any(|text| {
                    let text = text.to_lowercase();
                    text.contains(&hint)
                        || (!text.is_empty() && hint.contains(&text))
                        || words.iter().any(|word| text.contains(word))
                })
            })
            .take(10)
            .map(|entry| format!("  {}", entry.line()))
            .collect();
        if similar.is_empty() {
            format!("No named node looks like {hint:?}.")
        } else {
            format!("Named nodes like it:\n{}", similar.join("\n"))
        }
    }
}

fn intersects(a: &Bounds<Pixels>, b: &Bounds<Pixels>) -> bool {
    a.left() < b.right() && b.left() < a.right() && a.top() < b.bottom() && b.top() < a.bottom()
}

fn is_text_field(role: Role) -> bool {
    matches!(
        role,
        Role::TextInput
            | Role::MultilineTextInput
            | Role::SearchInput
            | Role::EmailInput
            | Role::NumberInput
            | Role::PasswordInput
            | Role::PhoneNumberInput
            | Role::UrlInput
    )
}

/// An element of the window found by what a user sees, to act on as a user
/// would. Got from [`VisualTestContext::get_by_role`] and its siblings.
///
/// Each action sends the accessibility action a screen reader or GUI driver
/// would, then lets the app settle: pending work runs, the window draws,
/// and frame callbacks the action waits for (a press below the fold waits
/// for the frame that scrolls it in) run, so a check right after sees the
/// result.
pub struct UiNode<'a> {
    cx: &'a mut VisualTestContext,
    entry: Entry,
}

impl<'a> UiNode<'a> {
    /// Its role.
    pub fn role(&self) -> Role {
        self.entry.role
    }

    /// Its accessible name: its label, or the text painted in it.
    pub fn name(&self) -> Option<&str> {
        self.entry.name.as_deref()
    }

    /// Its value, for a field that reports one.
    pub fn value(&self) -> Option<&str> {
        self.entry.value.as_deref()
    }

    /// Whether it holds the focus.
    pub fn is_focused(&self) -> bool {
        self.entry.focused
    }

    /// Whether it is disabled.
    pub fn is_disabled(&self) -> bool {
        self.entry.disabled
    }

    /// Whether any of it is inside the window and the scroll containers
    /// around it.
    pub fn is_shown(&self) -> bool {
        self.entry.shown
    }

    /// Clicks it at its centre, scrolling it into view first if it is
    /// scrolled out of its container. Panics when it is disabled.
    pub fn click(self) {
        self.refuse_disabled("click");
        self.act(Action::Click);
    }

    /// Moves the pointer onto it, so its hover style, hover listeners and
    /// tooltip show, then waits out the tooltip delay (a second of the test
    /// clock).
    pub fn hover(self) {
        let cx = self.act(Action::ShowTooltip);
        wait_out_delays(cx);
    }

    /// Moves the pointer off the window, ending a [`Self::hover`], then waits
    /// out the tooltip's hide delay.
    pub fn unhover(self) {
        let cx = self.act(Action::HideTooltip);
        wait_out_delays(cx);
    }

    /// Scrolls each scroll container around it until it is in view, as the
    /// wheel would.
    pub fn scroll_into_view(self) {
        self.act(Action::ScrollIntoView);
    }

    /// Focuses it. Panics when it has no focus handle the tree knows of
    /// (an element needs an id, a role and `track_focus`).
    pub fn focus(self) {
        self.focus_and_keep();
    }

    /// Focuses it, then types `text` into it as keystrokes.
    pub fn type_text(self, text: &str) {
        let cx = self.focus_and_keep();
        cx.simulate_input(text);
        settle(cx);
    }

    fn focus_and_keep(self) -> &'a mut VisualTestContext {
        if !self.entry.focusable {
            panic!(
                "{} can't take the focus: it has no focus handle. Give the element \
                 an id, a role and `track_focus`.",
                self.entry.line()
            );
        }
        self.refuse_disabled("focus");
        let id = self.entry.id;
        let line = self.entry.line();
        let cx = self.act(Action::Focus);
        let focused = cx.update(|window, cx| {
            window
                .a11y
                .focus_ids
                .get(&id)
                .and_then(|focus| crate::FocusHandle::for_id(*focus, &cx.focus_handles))
                .is_some_and(|handle| handle.is_focused(window))
        });
        assert!(focused, "{line} did not take the focus");
        cx
    }

    fn refuse_disabled(&self, what: &str) {
        if self.entry.disabled {
            panic!("can't {what} {}: it is disabled", self.entry.line());
        }
    }

    fn act(self, action: Action) -> &'a mut VisualTestContext {
        let id = self.entry.id;
        self.cx.update(|window, cx| {
            window.handle_a11y_action(
                accesskit::ActionRequest {
                    action,
                    target_tree: accesskit::TreeId::ROOT,
                    target_node: id,
                    data: None,
                },
                cx,
            )
        });
        settle(self.cx);
        self.cx
    }
}

/// Runs what is pending, draws, and delivers frames while a frame callback
/// waits, until the window is still.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..16 {
        cx.run_until_parked();
        let waiting = cx.update(|window, _| {
            window.invalidator.is_dirty() || !window.next_frame_callbacks.borrow().is_empty()
        });
        if !waiting {
            return;
        }
        cx.cx
            .test_window(cx.window)
            .simulate_frame_request(crate::RequestFrameOptions {
                require_presentation: true,
                ..Default::default()
            });
    }
}

/// Lets a tooltip's show or hide delay pass.
fn wait_out_delays(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(1));
    settle(cx);
}

impl VisualTestContext {
    /// Switches accessibility on for the window if it is not, and brings its
    /// tree up to date.
    fn a11y_snapshot(&mut self) -> Snapshot {
        let switched_on = self.update(|window, _| {
            assert!(
                window.a11y.is_enabled(),
                "accessibility is disabled for this window \
                 (`Application::new_inaccessible`), so it has no tree to test by"
            );
            let on = window
                .a11y
                .active_flag
                .load(std::sync::atomic::Ordering::SeqCst);
            if !on {
                crate::fast::a11y::set_active_flag(&window.a11y, true);
                window.refresh();
            }
            !on
        });
        settle(self);
        if switched_on {
            self.update(|window, _| window.refresh());
            settle(self);
        }
        self.update(|window, _| Snapshot::take(window))
    }

    fn node(&mut self, entry: Entry) -> UiNode<'_> {
        UiNode { cx: self, entry }
    }

    /// The one element with `role` named exactly `name`. Panics, listing the
    /// nodes named like it and the whole tree, when there is none or more.
    pub fn get_by_role(&mut self, role: Role, name: &str) -> UiNode<'_> {
        let entry = self.a11y_snapshot().the_one(
            &format!("{role:?} named {name:?}"),
            name,
            |e| e.role == role && e.name.as_deref() == Some(name),
        );
        self.node(entry)
    }

    /// The one element with `role` whose name contains `text`.
    pub fn get_by_role_containing(&mut self, role: Role, text: &str) -> UiNode<'_> {
        let entry = self.a11y_snapshot().the_one(
            &format!("{role:?} with {text:?} in its name"),
            text,
            |e| e.role == role && e.name.as_deref().is_some_and(|name| name.contains(text)),
        );
        self.node(entry)
    }

    /// The one element of any role named exactly `name`: a button by its
    /// text, a label by the text it shows.
    pub fn get_by_label(&mut self, name: &str) -> UiNode<'_> {
        let entry = self
            .a11y_snapshot()
            .the_one(&format!("node named {name:?}"), name, |e| {
                e.name.as_deref() == Some(name)
            });
        self.node(entry)
    }

    /// The one element of any role whose name contains `text`.
    pub fn get_by_label_containing(&mut self, text: &str) -> UiNode<'_> {
        let entry = self.a11y_snapshot().the_one(
            &format!("node with {text:?} in its name"),
            text,
            |e| e.name.as_deref().is_some_and(|name| name.contains(text)),
        );
        self.node(entry)
    }

    /// The element with `role` named exactly `name`, if there is one.
    /// Panics when there are more.
    pub fn query_by_role(&mut self, role: Role, name: &str) -> Option<UiNode<'_>> {
        let snapshot = self.a11y_snapshot();
        let matches = |e: &Entry| e.role == role && e.name.as_deref() == Some(name);
        if !snapshot.entries.iter().any(matches) {
            return None;
        }
        let entry = snapshot.the_one(&format!("{role:?} named {name:?}"), name, matches);
        Some(self.node(entry))
    }

    /// The element of any role named exactly `name`, if there is one.
    /// Panics when there are more.
    pub fn query_by_label(&mut self, name: &str) -> Option<UiNode<'_>> {
        let snapshot = self.a11y_snapshot();
        let matches = |e: &Entry| e.name.as_deref() == Some(name);
        if !snapshot.entries.iter().any(matches) {
            return None;
        }
        let entry = snapshot.the_one(&format!("node named {name:?}"), name, matches);
        Some(self.node(entry))
    }

    /// The one text field whose label or placeholder is exactly
    /// `label_or_placeholder`, or that a label node with that text labels.
    pub fn get_text_field(&mut self, label_or_placeholder: &str) -> UiNode<'_> {
        let snapshot = self.a11y_snapshot();
        let labels: Vec<NodeId> = snapshot
            .entries
            .iter()
            .filter(|e| e.name.as_deref() == Some(label_or_placeholder))
            .map(|e| e.id)
            .collect();
        let entry = snapshot.the_one(
            &format!("text field labelled {label_or_placeholder:?} or with that placeholder"),
            label_or_placeholder,
            |e| {
                is_text_field(e.role)
                    && (e.name.as_deref() == Some(label_or_placeholder)
                        || e.placeholder.as_deref() == Some(label_or_placeholder)
                        || e.labelled_by.iter().any(|id| labels.contains(id)))
            },
        );
        self.node(entry)
    }

    /// Focuses the text field found by its label or placeholder, as
    /// [`Self::get_text_field`] finds it, and types `text` into it.
    pub fn type_into(&mut self, label_or_placeholder: &str, text: &str) {
        self.get_text_field(label_or_placeholder).type_text(text);
    }

    /// Whether some node on screen shows `text` in its name or value.
    pub fn is_shown(&mut self, text: &str) -> bool {
        self.a11y_snapshot()
            .entries
            .iter()
            .any(|e| e.shown && e.texts().any(|shown| shown.contains(text)))
    }

    /// Panics, with the tree, unless some node on screen shows `text`.
    #[track_caller]
    pub fn assert_shown(&mut self, text: &str) {
        let snapshot = self.a11y_snapshot();
        let shown = |e: &Entry| e.texts().any(|t| t.contains(text));
        if snapshot.entries.iter().any(|e| e.shown && shown(e)) {
            return;
        }
        let off_screen = if snapshot.entries.iter().any(shown) {
            " (it is in the tree, but scrolled or clipped out of view)"
        } else {
            ""
        };
        panic!(
            "{text:?} is not on screen{off_screen}.\n{}\nThe window's accessibility tree:\n{}",
            snapshot.like(text),
            snapshot.dump()
        );
    }

    /// Panics, with the tree, if some node on screen shows `text`.
    #[track_caller]
    pub fn assert_not_shown(&mut self, text: &str) {
        let snapshot = self.a11y_snapshot();
        let found: Vec<String> = snapshot
            .entries
            .iter()
            .filter(|e| e.shown && e.texts().any(|t| t.contains(text)))
            .map(|e| format!("  {}   (in {})", e.line(), snapshot.ancestry(e)))
            .collect();
        if !found.is_empty() {
            panic!(
                "{text:?} is on screen:\n{}\nThe window's accessibility tree:\n{}",
                found.join("\n"),
                snapshot.dump()
            );
        }
    }

    /// The window's accessibility tree as text: one node a line, indented by
    /// depth, with its role, name, placeholder, value and whether it is
    /// focused, disabled or off-screen. For failure messages, and for
    /// finding out what a window offers to drive.
    pub fn a11y_tree_text(&mut self) -> String {
        self.a11y_snapshot().dump()
    }
}
