//! Memory a window gives back once it has drawn nothing for a while.
//!
//! A window keeps two frames: the one on screen and the one the next draw
//! fills, cleared but holding the lists the frame before needed, and the
//! element arena keeps chunks for the next draw. Both save allocating on every
//! frame while frames are drawn, and are worth nothing to a window that is
//! idle. A second without a frame frees them; the next frame allocates what
//! it needs again.

use crate::{App, AppContext as _, Window};
use std::time::Duration;

/// How long a window has to go without a frame before it gives memory back.
const IDLE: Duration = Duration::from_secs(1);

/// A window's idle trim: how many frames it drew, and whether a trim is
/// waiting for it to go idle.
#[derive(Default)]
pub(crate) struct IdleTrim {
    frames: u64,
    waiting: bool,
}

/// Called after every frame `window` draws.
pub(crate) fn frame_drawn(window: &mut Window, cx: &mut App) {
    let state = &mut window.fast_idle_trim;
    state.frames += 1;
    if state.waiting {
        return;
    }
    state.waiting = true;
    let handle = window.handle;
    cx.spawn(async move |cx| {
        let mut frames = None;
        loop {
            cx.background_executor().timer(IDLE).await;
            let idle = cx.update_window(handle, |_, window, cx| {
                let state = &mut window.fast_idle_trim;
                if frames == Some(state.frames) {
                    state.waiting = false;
                    release(window, cx);
                    true
                } else {
                    frames = Some(state.frames);
                    false
                }
            });
            if idle.unwrap_or(true) {
                break;
            }
        }
    })
    .detach();
}

/// Frees what only the next frame would use.
fn release(window: &mut Window, cx: &mut App) {
    let frame = &mut window.next_frame;
    frame.mouse_listeners = Vec::new();
    frame.hitboxes = Vec::new();
    frame.accessed_element_states = Vec::new();
    frame.dispatch_tree.nodes = Vec::new();
    frame.retained.records = Vec::new();
    frame.retained.by_id.shrink_to_fit();
    frame.element_states.shrink_to_fit();
    let scene = &mut frame.scene;
    scene.paint_operations = Vec::new();
    scene.shadows = Vec::new();
    scene.quads = Vec::new();
    scene.paths = Vec::new();
    scene.underlines = Vec::new();
    scene.monochrome_sprites = Vec::new();
    scene.subpixel_sprites = Vec::new();
    scene.polychrome_sprites = Vec::new();
    crate::fast::arena::release_idle(&mut cx.element_arena.borrow_mut());
}
