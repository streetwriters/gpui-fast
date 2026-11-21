//! The CPU renderer's output for fixed scenes, as hashes. The hashes were taken
//! from the renderer before it drew in parallel and cropped, and before the
//! shaders stopped repeating per-row work, so a change to the shader maths, or
//! to the order they are applied in, fails these tests.

mod common;

/// The hash of `common::scene()` drawn whole, by the single-threaded renderer.
const GOLDEN: u64 = 5000237420952691639;

/// The hash of `common::scene_extras()`, taken from the renderer as it was
/// before the per-row hoisting.
const GOLDEN_EXTRAS: u64 = 12881127138112419967;

#[test]
fn the_scene_is_drawn_byte_for_byte_as_before() {
    let image = common::render();
    assert_eq!(
        common::fingerprint(&image),
        GOLDEN,
        "the rendering of the scene changed"
    );
}

#[test]
fn the_extra_scene_is_drawn_byte_for_byte_as_before() {
    let image = common::render_extras();
    assert_eq!(
        common::fingerprint(&image),
        GOLDEN_EXTRAS,
        "the rendering of the extra scene changed"
    );
}
