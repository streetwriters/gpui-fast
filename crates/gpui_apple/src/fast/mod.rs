//! Everything gpui-fast adds to the Metal renderer and text system lives under
//! this module.
//!
//! The rest of this crate is kept as close to upstream GPUI (Zed's
//! `crates/gpui_apple`) as possible, so that pulling in upstream changes stays
//! a matter of merging rather than of rewriting. Upstream files only *call
//! into* this module: a field holding this module's state, a line forwarding a
//! call to it. The logic itself — data structures, algorithms, bookkeeping,
//! tests — is written here, one file per topic.

#[path = "../../../gpui/src/fast/buffer_shrink.rs"]
pub(crate) mod buffer_shrink;
pub(crate) mod composition;
pub(crate) mod dynamic_textures;
pub(crate) mod effects;
#[cfg(feature = "font-kit")]
pub(crate) mod glyph_coverage;
pub(crate) mod instance_buffers;
#[path = "../../../gpui/src/fast/layer_budget.rs"]
pub(crate) mod layer_budget;
pub(crate) mod layers;
pub(crate) mod path_textures;
pub(crate) mod paths;
pub(crate) mod rounded_clip;
pub(crate) mod shared_device;
#[cfg(feature = "font-kit")]
pub(crate) mod text_system;
