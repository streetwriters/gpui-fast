//! A small app driven only through the user-level test API
//! (`crate::fast::ui_test`): by roles and the text a user reads, with no
//! element ids, debug selectors or pixel positions in the tests.

use accesskit::Role;

use crate::{
    Context, FocusHandle, IntoElement, KeyDownEvent, Render, TestAppContext, Window, div,
    prelude::*, px,
};

/// A mail-like window: folders to pick, a Save button that says when it
/// saved, a search field, a help icon with a tooltip, and a scrolling list.
struct Sample {
    folder: &'static str,
    saved: bool,
    query: String,
    opened: Option<usize>,
    search: FocusHandle,
}

impl Sample {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            folder: "Inbox",
            saved: false,
            query: String::new(),
            opened: None,
            search: cx.focus_handle(),
        }
    }
}

struct Tip(&'static str);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(self.0)
    }
}

impl Render for Sample {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let folders = ["Inbox", "Drafts", "Sent"].map(|folder| {
            div()
                .id(folder)
                .h(px(20.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.folder = folder;
                    cx.notify();
                }))
                .child(folder)
        });
        let search = div()
            .id("search")
            .role(Role::TextInput)
            .aria_placeholder("Search")
            .aria_value(self.query.clone())
            .track_focus(&self.search)
            .h(px(20.))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if let Some(text) = &event.keystroke.key_char {
                    this.query.push_str(text);
                    cx.notify();
                }
            }))
            .child(if self.query.is_empty() {
                "Search".to_string()
            } else {
                self.query.clone()
            });
        let list = div()
            .id("messages")
            .h(px(100.))
            .overflow_y_scroll()
            .children((0..30).map(|ix| {
                div()
                    .id(ix)
                    .h(px(20.))
                    .flex_none()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.opened = Some(ix);
                        cx.notify();
                    }))
                    .child(format!("Message {ix}"))
            }));
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(folders)
            .child(format!("Showing {}", self.folder))
            .child(
                div()
                    .id("save")
                    .h(px(20.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.saved = true;
                        cx.notify();
                    }))
                    .child("Save"),
            )
            .when(self.saved, |this| this.child("Saved"))
            .child(search)
            .child(
                div()
                    .id("help")
                    .h(px(20.))
                    .tooltip(|_, cx| cx.new(|_| Tip("Opens the manual")).into())
                    .child("Help"),
            )
            .child(list)
            .when_some(self.opened, |this, ix| {
                this.child(format!("Opened message {ix}"))
            })
    }
}

#[crate::test]
fn clicking_save_says_saved(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| Sample::new(cx));
    cx.assert_not_shown("Saved");
    cx.get_by_role(Role::Button, "Save").click();
    cx.assert_shown("Saved");
}

#[crate::test]
fn picking_a_folder_shows_it(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| Sample::new(cx));
    cx.assert_shown("Showing Inbox");
    cx.get_by_role(Role::Button, "Drafts").click();
    cx.assert_shown("Showing Drafts");
    cx.assert_not_shown("Showing Inbox");
}

#[crate::test]
fn typing_into_the_search_field(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| Sample::new(cx));
    cx.type_into("Search", "hello");
    let field = cx.get_text_field("Search");
    assert!(field.is_focused());
    assert_eq!(field.value(), Some("hello"));
    cx.assert_shown("hello");
}

#[crate::test]
fn hovering_help_shows_its_tooltip(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| Sample::new(cx));
    cx.assert_not_shown("Opens the manual");
    cx.get_by_label("Help").hover();
    cx.get_by_role(Role::Tooltip, "Opens the manual");
    cx.get_by_label("Help").unhover();
    cx.assert_not_shown("Opens the manual");
}

#[crate::test]
fn a_message_below_the_fold_is_scrolled_to_and_opened(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| Sample::new(cx));
    cx.assert_shown("Message 0");
    cx.assert_not_shown("Message 25");

    cx.get_by_role(Role::Button, "Message 25").scroll_into_view();
    cx.assert_shown("Message 25");
    cx.assert_not_shown("Message 0");

    // A press on a message out of view scrolls to it first.
    cx.get_by_role(Role::Button, "Message 2").click();
    cx.assert_shown("Opened message 2");
}

#[crate::test]
fn focusing_the_search_field(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| Sample::new(cx));
    assert!(!cx.get_text_field("Search").is_focused());
    cx.get_text_field("Search").focus();
    assert!(cx.get_text_field("Search").is_focused());
    assert!(cx.a11y_tree_text().contains("TextInput placeholder=\"Search\" value=\"\" [focused]"));
}

/// Panics with `f`'s message.
fn panic_message(f: impl FnOnce()) -> String {
    let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .expect_err("expected a panic");
    error
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| error.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[crate::test]
fn a_missing_element_lists_the_names_like_it_and_the_tree(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| Sample::new(cx));
    let message = panic_message(|| {
        cx.get_by_role(Role::Button, "save");
    });
    assert!(message.contains("no Button named \"save\""), "{message}");
    assert!(message.contains("Named nodes like it:\n  Button \"Save\""), "{message}");
    assert!(message.contains("The window's accessibility tree:"), "{message}");
    assert!(message.contains("\nWindow\n  Button \"Inbox\"\n"), "{message}");
}

#[crate::test]
fn an_ambiguous_name_lists_every_match(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| Sample::new(cx));
    let message = panic_message(|| {
        cx.get_by_role_containing(Role::Button, "Message 1");
    });
    // Message 1 and Message 10 to 19.
    assert!(message.starts_with("11 matches for Button with \"Message 1\""), "{message}");
    assert!(message.contains("  Button \"Message 19\" [off-screen]   (in Window"), "{message}");
    assert!(cx.query_by_role(Role::Button, "Message 31").is_none());
    assert!(cx.query_by_label("Help").is_some());
}

#[crate::test]
fn the_tree_reads_as_text(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| Sample::new(cx));
    let tree = cx.a11y_tree_text();
    let head: Vec<&str> = tree.lines().take(9).collect();
    assert_eq!(
        head,
        [
            "Window",
            "  Button \"Inbox\"",
            "  Button \"Drafts\"",
            "  Button \"Sent\"",
            "  Label \"Showing Inbox\"",
            "  Button \"Save\"",
            "  TextInput placeholder=\"Search\" value=\"\"",
            "    Label \"Search\"",
            "  Group",
        ],
        "{tree}"
    );
}
