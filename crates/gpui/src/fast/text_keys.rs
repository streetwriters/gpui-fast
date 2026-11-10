//! Keys a focused text input takes as text.
//!
//! GPUI offers a key down to the key listeners of every element on the way to
//! the focused one, and only then, if none stopped it, lets the platform insert
//! the character through the input handler. An ancestor that listens for keys
//! therefore sees every character typed into a text field below it, and cannot
//! stop one without also stopping the insertion. A key the focused element's
//! input handler is about to take as text is offered to the focused element's
//! own listeners alone: [`listener_path`] cuts the path down to it. The
//! insertion through the input handler is untouched.

use crate::key_dispatch::DispatchNodeId;
use crate::{App, KeyDownEvent, Window};
use smallvec::SmallVec;
use std::any::Any;

/// The dispatch nodes whose key listeners see `event`: the whole `path` to the
/// focused element, or only the focused element when its input handler will
/// take the key as text.
pub(crate) fn listener_path(
    window: &mut Window,
    event: &dyn Any,
    path: &SmallVec<[DispatchNodeId; 32]>,
    cx: &mut App,
) -> SmallVec<[DispatchNodeId; 32]> {
    if path.len() > 1 && is_taken_as_text(window, event, cx) {
        return path.last().copied().into_iter().collect();
    }
    path.clone()
}

fn is_taken_as_text(window: &mut Window, event: &dyn Any, cx: &mut App) -> bool {
    let Some(event) = event.downcast_ref::<KeyDownEvent>() else {
        return false;
    };
    let keystroke = &event.keystroke;
    let modifiers = keystroke.modifiers;
    let types_text = keystroke
        .key_char
        .as_deref()
        .is_some_and(|text| !text.is_empty() && text.chars().all(|c| !c.is_control()));
    if !types_text || modifiers.control || modifiers.platform || modifiers.function {
        return false;
    }
    let Some(mut handler) = window.platform_window.take_input_handler() else {
        return false;
    };
    let accepts = handler.accepts_text_input(window, cx);
    window.platform_window.set_input_handler(handler);
    accepts
}

/// What an input method does to the focused element's input handler, for tests
/// that cannot reach the platform: [`Window::simulate_marked_text`] shows text
/// being composed and [`Window::simulate_committed_text`] replaces it with the
/// final text.
#[cfg(any(test, feature = "test-support"))]
impl Window {
    /// Marks `text` as being composed in the focused element's input handler.
    /// Returns whether there was a handler.
    pub fn simulate_marked_text(&mut self, text: &str, cx: &mut App) -> bool {
        let Some(mut handler) = self.platform_window.take_input_handler() else {
            return false;
        };
        handler
            .handler
            .replace_and_mark_text_in_range(None, text, None, self, cx);
        self.platform_window.set_input_handler(handler);
        true
    }

    /// Replaces the text being composed, or the selection, with `text` and
    /// ends the composition. Returns whether there was a handler.
    pub fn simulate_committed_text(&mut self, text: &str, cx: &mut App) -> bool {
        let Some(mut handler) = self.platform_window.take_input_handler() else {
            return false;
        };
        let marked = handler.handler.marked_text_range(self, cx);
        handler
            .handler
            .replace_text_in_range(marked, text, self, cx);
        handler.handler.unmark_text(self, cx);
        self.platform_window.set_input_handler(handler);
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, Keystroke,
        ParentElement as _, Render, Styled as _, TestAppContext, Window, canvas, div, px,
    };
    use std::{cell::Cell, ops::Range, rc::Rc};

    struct Field {
        focus: crate::FocusHandle,
        accepts: bool,
        outer: Rc<Cell<usize>>,
        own: Rc<Cell<usize>>,
        inserted: Rc<Cell<usize>>,
    }

    impl Render for Field {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let (focus, field) = (self.focus.clone(), cx.entity());
            let (outer, own) = (self.outer.clone(), self.own.clone());
            div()
                .on_key_down(move |_, _, _| outer.set(outer.get() + 1))
                .child(
                    div()
                        .track_focus(&self.focus)
                        .on_key_down(move |_, _, _| own.set(own.get() + 1))
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, cx| {
                                    window.handle_input(
                                        &focus,
                                        crate::ElementInputHandler::new(bounds, field),
                                        cx,
                                    )
                                },
                            )
                            .w(px(100.))
                            .h(px(20.)),
                        ),
                )
        }
    }

    impl crate::EntityInputHandler for Field {
        fn text_for_range(
            &mut self,
            _: Range<usize>,
            _: &mut Option<Range<usize>>,
            _: &mut Window,
            _: &mut Context<Self>,
        ) -> Option<String> {
            None
        }
        fn selected_text_range(
            &mut self,
            _: bool,
            _: &mut Window,
            _: &mut Context<Self>,
        ) -> Option<crate::UTF16Selection> {
            None
        }
        fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
            None
        }
        fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
        fn replace_text_in_range(
            &mut self,
            _: Option<Range<usize>>,
            _: &str,
            _: &mut Window,
            _: &mut Context<Self>,
        ) {
            self.inserted.set(self.inserted.get() + 1);
        }
        fn replace_and_mark_text_in_range(
            &mut self,
            _: Option<Range<usize>>,
            _: &str,
            _: Option<Range<usize>>,
            _: &mut Window,
            _: &mut Context<Self>,
        ) {
        }
        fn bounds_for_range(
            &mut self,
            _: Range<usize>,
            _: crate::Bounds<crate::Pixels>,
            _: &mut Window,
            _: &mut Context<Self>,
        ) -> Option<crate::Bounds<crate::Pixels>> {
            None
        }
        fn character_index_for_point(
            &mut self,
            _: crate::Point<crate::Pixels>,
            _: &mut Window,
            _: &mut Context<Self>,
        ) -> Option<usize> {
            None
        }
        fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
            self.accepts
        }
    }

    fn type_key(accepts: bool, key: &str, key_char: Option<&str>) -> (usize, usize, usize) {
        let mut cx = TestAppContext::single();
        let (outer, own, inserted) = (
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
        );
        let window = cx.add_window({
            let (outer, own, inserted) = (outer.clone(), own.clone(), inserted.clone());
            move |_, cx| Field {
                focus: cx.focus_handle(),
                accepts,
                outer,
                own,
                inserted,
            }
        });
        let field: Entity<Field> = window.entity(&cx).unwrap();
        let focus = field.read_with(&cx, |field, _| field.focus.clone());
        cx.update_window(window.into(), |_, window, cx| {
            window.focus(&focus, cx);
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        let keystroke = Keystroke {
            modifiers: Default::default(),
            key: key.to_string(),
            key_char: key_char.map(str::to_string),
        };
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_keystroke(keystroke, cx);
        })
        .unwrap();
        (outer.get(), own.get(), inserted.get())
    }

    #[test]
    fn a_character_the_input_takes_is_not_offered_to_ancestors_and_is_still_inserted() {
        assert_eq!(type_key(true, "a", Some("a")), (0, 1, 1));
    }

    #[test]
    fn an_input_method_reaches_the_focused_input_handler_without_key_events() {
        let mut cx = TestAppContext::single();
        let (outer, own, inserted) = (
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
        );
        let window = cx.add_window({
            let (outer, own, inserted) = (outer.clone(), own.clone(), inserted.clone());
            move |_, cx| Field {
                focus: cx.focus_handle(),
                accepts: true,
                outer,
                own,
                inserted,
            }
        });
        let field: Entity<Field> = window.entity(&cx).unwrap();
        let focus = field.read_with(&cx, |field, _| field.focus.clone());
        let composed = cx
            .update_window(window.into(), |_, window, cx| {
                window.focus(&focus, cx);
                window.draw(cx).clear(cx);
                window.draw(cx).clear(cx);
                (
                    window.simulate_marked_text("に", cx),
                    window.simulate_committed_text("日本", cx),
                )
            })
            .unwrap();
        assert_eq!(composed, (true, true));
        assert_eq!((outer.get(), own.get(), inserted.get()), (0, 0, 1));
    }

    #[test]
    fn a_key_that_types_nothing_goes_to_ancestors() {
        assert_eq!(type_key(true, "f2", None), (1, 1, 0));
    }

    #[test]
    fn a_character_goes_to_ancestors_when_the_focused_element_does_not_take_text() {
        assert_eq!(type_key(false, "a", Some("a")).0, 1);
    }
}
