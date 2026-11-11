//! A window keeps the frame it was given while a test driver runs the app.
//!
//! AppKit constrains a titled window to the visible frame of its screen when it
//! is shown. A test that opens a window larger than the display, as a CI
//! runner's 1024x768 virtual display is for most of them, would otherwise get a
//! smaller window than it asked for, and what lies beyond the screen would
//! not be visible to it.

use objc2::{
    class, msg_send,
    runtime::{AnyClass, AnyObject, ClassBuilder, Sel},
    sel,
};
use objc2_foundation::NSRect;

pub(crate) unsafe fn register(decl: &mut ClassBuilder, superclass: &AnyClass) {
    extern "C" fn window(
        this: &AnyObject,
        _: Sel,
        frame: NSRect,
        screen: *mut AnyObject,
    ) -> NSRect {
        if crate::fast::test_active::in_background() {
            return frame;
        }
        unsafe {
            msg_send![super(this, class!(NSWindow)), constrainFrameRect: frame, toScreen: screen]
        }
    }
    extern "C" fn panel(this: &AnyObject, _: Sel, frame: NSRect, screen: *mut AnyObject) -> NSRect {
        if crate::fast::test_active::in_background() {
            return frame;
        }
        unsafe {
            msg_send![super(this, class!(NSPanel)), constrainFrameRect: frame, toScreen: screen]
        }
    }
    unsafe {
        if superclass == class!(NSPanel) {
            decl.add_method(
                sel!(constrainFrameRect:toScreen:),
                panel as extern "C" fn(_, Sel, NSRect, *mut AnyObject) -> NSRect,
            );
        } else {
            decl.add_method(
                sel!(constrainFrameRect:toScreen:),
                window as extern "C" fn(_, Sel, NSRect, *mut AnyObject) -> NSRect,
            );
        }
    }
}
