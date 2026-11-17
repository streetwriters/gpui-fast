//! Frame buffers sized to the frame they hold.
//!
//! A frame's lists grow by doubling while it is drawn and are cleared, not
//! freed, for the frame after the next one to reuse, so a window holds two
//! sets of them at up to twice what a frame needs, and after a large frame it
//! holds a large frame's worth for good. Once a frame is drawn, each list
//! much larger than it needs is cut to fit. A frame as large as the one
//! before it fills its lists without growing them, so this costs nothing in
//! a steady state; a frame that grew a list pays for one copy of it.

use crate::window::Frame;

/// Shrinks `list` to its length when more than a quarter of it is unused.
fn fit<T>(list: &mut Vec<T>) {
    const SLACK_FLOOR: usize = 64;
    if list.capacity() > list.len() + list.len() / 4 + SLACK_FLOOR {
        list.shrink_to_fit();
    }
}

/// Cuts the lists of `frame`, which was just drawn, to what it holds.
pub(crate) fn fit_frame(frame: &mut Frame) {
    fit(&mut frame.mouse_listeners);
    fit(&mut frame.hitboxes);
    fit(&mut frame.accessed_element_states);
    fit(&mut frame.dispatch_tree.nodes);
    fit(&mut frame.retained.records);
    let scene = &mut frame.scene;
    fit(&mut scene.paint_operations);
    fit(&mut scene.shadows);
    fit(&mut scene.quads);
    fit(&mut scene.paths);
    fit(&mut scene.underlines);
    fit(&mut scene.monochrome_sprites);
    fit(&mut scene.subpixel_sprites);
    fit(&mut scene.polychrome_sprites);
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn cuts_a_list_a_quarter_unused() {
        let mut list: Vec<u64> = Vec::with_capacity(4096);
        list.extend(0..1000);
        fit(&mut list);
        assert_eq!(list.capacity(), 1000);
    }

    #[test]
    fn leaves_a_list_that_is_nearly_full() {
        let mut list: Vec<u64> = Vec::with_capacity(1100);
        list.extend(0..1000);
        fit(&mut list);
        assert_eq!(list.capacity(), 1100);
    }
}
