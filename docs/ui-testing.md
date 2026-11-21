# Testing a window by what a user sees

With the `test-support` feature, a `VisualTestContext` can drive its window
the way a user does, or a GUI driver like Cua: find a button by its text,
click it, check that a message appeared. No element ids, debug selectors or
pixel positions. It is modelled on
[egui_kittest](https://github.com/emilk/egui/tree/main/crates/egui_kittest)'s
`get_by_label` and `click`.

It reads the window's accessibility tree, the one screen readers read, and
acts through the accessibility actions they send. The first call on a window
switches accessibility on for it; the test does nothing to turn it on.

```rust
use gpui::{Role, TestAppContext};

#[gpui::test]
fn saving_says_saved(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| Editor::new(cx));

    cx.assert_not_shown("Saved");
    cx.get_by_role(Role::Button, "Save").click();
    cx.assert_shown("Saved");

    cx.type_into("Search", "hello");
    cx.get_by_label("Help").hover();
    cx.assert_shown("Opens the manual");

    cx.get_by_role(Role::Button, "Message 25").click(); // scrolls to it first
    cx.assert_shown("Opened message 25");
}
```

The tests in `crates/gpui/src/fast/tests/ui_test.rs` drive a small sample
window (folders, a Save button, a search field, a tooltip, a scrolling
list) through nothing else.

## What is in the tree

- Something clickable is a **button** named by the text painted in it, unless
  it has a role or an `aria_label` of its own.
- Plain text painted outside such a node is a **label** named by its text,
  so `assert_shown("Saved")` and `get_by_label("Help")` find it. A label
  can be clicked and hovered: the pointer goes to its middle, as a user's
  would.
- A **text field** is an element with `.id(..)`, `.role(Role::TextInput)`
  (or another text-input role), `.track_focus(..)` and an `aria_label` or
  `aria_placeholder`; it is found by either, or by a label node it names
  with `labelled_by`.
- A scroll container is a **scroll view**; an element that only reacts to
  hover is a **group**; a tooltip on show is a **tooltip** named by its text.

## Finding

| Call | Finds |
| --- | --- |
| `get_by_role(role, name)` | the one node with that role and exactly that name |
| `get_by_role_containing(role, text)` | the one node with that role whose name contains `text` |
| `get_by_label(name)` | the one node of any role with exactly that name |
| `get_by_label_containing(text)` | the one node of any role whose name contains `text` |
| `query_by_role`, `query_by_label` | `Some` node, or `None` when there is none |
| `get_text_field(label_or_placeholder)` | the one text field with that label or placeholder |

When nothing matches, or more than one node does, the `get_*` calls panic
with the nodes named like what was asked for, every match with the nodes
around it, and the whole tree:

```text
no Button named "save" in the window.
Named nodes like it:
  Button "Save"
The window's accessibility tree:
Window
  Button "Inbox"
  ...
```

## Acting

Each returns once the app has settled: pending work ran, the window drew,
and frame callbacks the action waits for ran.

| On a `UiNode` | Does |
| --- | --- |
| `click()` | presses it at its middle, scrolling it into view first; panics if disabled |
| `hover()` / `unhover()` | moves the pointer onto it / off the window, then lets a second of the test clock pass for the tooltip delay |
| `scroll_into_view()` | scrolls each scroll container around it, as the wheel does |
| `focus()` | focuses it; panics if it has no focus handle |
| `type_text(text)` | focuses it, then types `text` as keystrokes |
| `name()`, `value()`, `role()`, `is_focused()`, `is_disabled()`, `is_shown()` | reads it |

`cx.type_into(label_or_placeholder, text)` is `get_text_field` then
`type_text`.

## Checking

- `assert_shown(text)` passes when a node on screen has `text` in its name
  or value; `assert_not_shown(text)` when none does. `is_shown(text)` is the
  bool. "On screen" means some of the node is inside the window and inside
  every scroll container around it.
- `a11y_tree_text()` is the tree as text, one node a line, indented by
  depth:

```text
Window
  Button "Inbox"
  Label "Showing Inbox"
  Button "Save"
  TextInput placeholder="Search" value="" [focused]
    Label "Search"
  Group
    Label "Help"
  ScrollView
    Button "Message 0"
    Button "Message 5" [off-screen]
```

Print it to find out what a window offers to drive.

## Screenshots

A headless test can render its window to a PNG, for a person or an agent to
look at. No window opens and no screen-recording permission is needed: the
frame is drawn by the platform's offscreen renderer (Metal on macOS, wgpu on
Linux), or by a software renderer when there is no GPU (see
[The CPU renderer](#the-cpu-renderer)). There is no pixel-diff API; assert on
pixels yourself if you must.

```rust
#[gpui::test(render)]
fn the_inbox_looks_right(cx: &mut TestAppContext) {
    cx.set_asset_source(Assets); // only if the view paints SVG icons
    let (_view, cx) = cx.add_window_view(|_, _| Inbox::new());
    let path = cx.screenshot("inbox").unwrap();          // the whole window
    cx.screenshot_element("save", Role::Button, "Save").unwrap(); // one element
    println!("{}", path.display());
}
```

- Opt in with `#[gpui::test(render)]`, or `cx.enable_rendering(gpui_platform::test_rendering())`
  before the first window opens. A test that does not opt in is unchanged and
  pays nothing; its `screenshot` fails saying rendering is not enabled.
- The crate needs `gpui_platform` as a dev-dependency with the `test-support`
  feature. On macOS without the Metal toolchain (`xcrun metal`) also turn on
  `runtime_shaders`.
- `screenshot(name)` writes `<folder>/<test function>/<name>.png` and returns
  the path. The folder is `$GPUI_SCREENSHOT_DIR`, else `gpui-screenshots` in
  the cargo target directory. Names are made safe as file names (anything but
  letters, digits, `-`, `_`, `.`, space becomes `_`).
- `screenshot_element(name, role, label)` crops to the bounds of the one element
  found by role and name in the accessibility tree. It fails naming the role
  and label when none or several match. The CPU renderer draws only the
  element's pixels; the GPU renderers draw the frame and crop it.
  `Window::render_region_to_image(region)` (`test-support`) does the same for a
  region in device pixels.
- `screenshot_image()` and `screenshot_element_image(role, label)` return the
  `RgbaImage` without writing it, for pixel assertions. The image is the
  window size times its scale factor (2.0 by default;
  `simulate_scale_factor_change` changes it).
- Text is real: rendering swaps the test's no-op text system for the
  platform's, shared by every test in the process.
- SVG icons need an asset source: `cx.set_asset_source(..)` gives
  `svg().path(..)` and `img(..)` somewhere to load from. A frame that paints an
  SVG with no asset fails the screenshot, naming the path, instead of leaving
  a blank space. (A malformed SVG or a missing `img` file is not detected.)
- Windows has no GPU offscreen renderer, so `test_rendering()` gives it the CPU
  one. On the web (wasm) there is none and `screenshot` fails saying so.

Cost, on macOS Metal in a debug build: about 5 ms to set up the renderer for a
window, about 5 ms to render a frame and about 40 ms more to encode and write
the PNG. Every window gets its own renderer, so tests run in parallel
(tested with four at once) and one test can open many windows (tested with 12)
without sharing a GPU device.

## The CPU renderer

`crates/gpui_cpu_renderer` draws a frame on the CPU, with
[tiny-skia](https://github.com/linebender/tiny-skia)'s pixmap as the frame
buffer. Where a GPU can't be used (a sandbox, a container, much of Linux CI)
`#[gpui::test(render)]` and `gpui_platform::test_rendering()` use it instead of
failing with "accessing a compatible Metal device". It uses the same text system
and the same atlas of glyph, icon and image tiles as the GPU renderers, so
layout, colours and text come out the same.

**When it is used**

- By default: only when the platform's GPU renderer can't be created. A machine
  with a working GPU keeps drawing with it. The fallback covers creating the
  renderer, not a GPU that fails later.
- Forced: `GPUI_TEST_RENDERER=cpu cargo test ...` draws every test on the CPU,
  GPU or not, so the images are the same on every machine. Any other value, or
  none, is the default. Use it for screenshots that are compared across
  machines.
- By hand: `cx.enable_rendering(gpui_platform::cpu_test_rendering())` picks the
  CPU renderer for one test, and `gpui_platform::gpu_test_rendering()` the GPU one
  with no fallback.
- In a sandbox on macOS without the Metal toolchain, build with the
  `runtime_shaders` feature as well; the CPU renderer needs no shaders, but
  `gpui_platform` still builds the Metal one.

**How close it is.** Every shader is ported to a per-pixel function (quads,
borders and corner radii, gradients, the analytic shadow, underlines, sprites,
paths), so the result is near the Metal one. The test
`crates/gpui_cpu_renderer/tests/cpu_vs_gpu.rs` draws a card with a shadow,
gradients (sRGB and Oklab), per-side and dashed borders, a slash pattern, a
checkerboard, an inset shadow, a rounded `overflow_hidden` clip, a scrolled
list, an icon, text and a wavy underline with both renderers and writes
`cpu.png`, `gpu.png`, `difference.png` and `report.txt`
(`cargo test -p gpui_cpu_renderer --features runtime_shaders --test cpu_vs_gpu -- --nocapture`;
without a GPU it says so and passes). On Metal, in an 800 x 760 image, the mean
difference is 0.007 out of 255, the largest difference of any channel is 3 and
no pixel is off by more than 16. Only the two gradients differ at all, in about
15% of their pixels, by up to 3 levels: Metal's gradient noise comes from a
`sin` that runs under fast-math, which the CPU one can't reproduce bit for bit.
Everything else (text, icons, borders, patterns, shadows, clips, the list) is
identical or one level off at a handful of pixels. The shader maths is a third
copy of `shaders.metal` and `shaders.wgsl`: on a machine with a GPU the test
fails when one element is off by more than 16 levels at over 1% of its pixels,
which is how a drifted copy shows. Do not assert on a single pixel along an
edge or in a gradient across renderers.

**How fast it is.** A primitive of 16K pixels or more is drawn in bands of
rows, spread over rayon's pool (`fast/bands.rs`); the per-pixel maths is
untouched, so the bytes are the same as one thread's
(`tests/golden.rs` pins the hashes of two fixed scenes taken before the
change). A region (`render_scene_region_to_image`, `fast/crop.rs`) draws into a
frame the size of the region only, so an element's capture costs the element,
not the window.

Work that is the same for every pixel of a row is done once per row, with the
same operations in the same order: a shadow's corner radii, blur weights and
sample offsets, a gradient's direction and a pattern's stripe set-up
(`shade.rs`). The inside of an opaque one-colour quad (no border or rounded
corner, a rectangular mask, no transform or colour filter) is written as a run
of bytes per row, the run's ends being found with the same comparisons the
per-pixel path makes (`fast/spans.rs`); a row it can't be sure of is drawn
pixel by pixel. A unit test draws random scenes with spans on and off and
compares the bytes.

`cargo test -p gpui_cpu_renderer --release --features runtime_shaders --test
timing -- --ignored --nocapture` prints the cost: on 12 cores a 2048 x 1536
frame of boxes takes about 105 ms on one thread before the bands and the
hoisting, 46 ms with the hoisting, 14 ms on all cores, and a 200 x 100 crop of
it 0.3 ms. The `what_the_tester_specs_cost` test in the same file
models the captures of the tester's `side-borders`, `outline-radius` and
`rounded-clip` e2e specs (their elements, at 1024 x 768 points and scale 2, one
capture per call the spec makes) and prints the whole-frame and element costs
for each; it is a model of the specs, since the tester's driver does not ask
for a crop yet.

**What it doesn't draw** (each is skipped with one warning per process, and
the frame is otherwise drawn)

- Surfaces (video, camera and screen-capture frames), external textures and
  dynamic textures.
- Effect groups: opacity, blur and drop-shadow filters on a group of elements,
  and blend modes. The group is not composited, so what is inside it can be
  missing from the image. Don't screenshot such a view on the CPU renderer.

Scroll layers (the retained-scroll fast path) are drawn: each tile is drawn
into its own bitmap over the layer's background and composited, as Metal does,
so a scrolled list shows its content. A tile is drawn only when its sprite
reaches the canvas, so a region of a scrolled list draws the tiles that show in
it, not all of them.

Borders have one colour for all sides, as in GPUI, with the width set per side.

## What it can't see yet

- Text an element paints itself (`ShapedLine::paint` in a custom element,
  a canvas, an image or SVG) is not text in the tree.
- "On screen" knows the window and scroll containers. It does not know
  about `overflow_hidden` clipping, zero opacity, `invisible()` or an
  overlay drawn on top.
- GPUI has no text field of its own, so a field is found only if its
  element gives itself a text-input role and a label or placeholder. One
  that does not is reachable only through the labels of the text it paints.
- A `div` has no way to say it is disabled; `[disabled]` shows only for
  elements whose `write_a11y_info` sets it.
- `hover()` and `unhover()` advance the test clock by a second, which runs
  any other timer due by then.
