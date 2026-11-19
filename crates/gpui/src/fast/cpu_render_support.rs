//! Read access to the scene fields that are private to this crate, for the CPU
//! renderer that `gpui_platform` provides to tests. The GPU renderers copy these
//! structs as bytes and never need it.

use crate::{Background, Hsla, LinearColorStop, PaddedBool32};

/// What a [`Background`] is made of, as the shaders read it.
#[derive(Clone, Copy, Debug)]
pub struct FillParts {
    /// 0 solid, 1 linear gradient, 2 slash pattern, 3 checkerboard.
    pub tag: u32,
    /// Whether a gradient mixes its stops in Oklab rather than sRGB.
    pub oklab: bool,
    /// The colour of a solid fill or a pattern.
    pub solid: Hsla,
    /// The angle in degrees of a gradient, the size of a checkerboard square, or
    /// the packed width and interval of a pattern.
    pub angle_or_pattern: f32,
    /// The two stops of a gradient.
    pub stops: [LinearColorStop; 2],
}

impl Background {
    /// The parts of this fill.
    pub fn fill_parts(&self) -> FillParts {
        FillParts {
            tag: self.tag as u32,
            oklab: self.color_space == crate::ColorSpace::Oklab,
            solid: self.solid,
            angle_or_pattern: self.gradient_angle_or_pattern_height,
            stops: self.colors,
        }
    }
}

impl PaddedBool32 {
    /// Whether the flag is set.
    pub fn is_set(&self) -> bool {
        self.0 != 0
    }
}
