//! GPU textures produced outside GPUI's sprite atlas: imported dma-bufs
//! (Linux) or test images. Renderers sample them directly instead of uploading
//! pixels into the atlas, so no CPU copy is involved.

use std::{
    any::Any,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::Size;

/// A stable identifier for an [`ExternalTexture`]; renderers cache their own
/// views and bind groups against it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExternalTextureId(pub u64);

/// An opaque GPU texture handed to GPUI to paint.
///
/// The payload is renderer-private (a `wgpu::Texture` for the WGPU renderer);
/// the platform produces it — through
/// [`Window::external_texture_from_rgba`](crate::Window::external_texture_from_rgba)
/// today, or an imported dma-buf — and the renderer downcasts it when painting.
#[derive(Clone)]
pub struct ExternalTexture {
    id: ExternalTextureId,
    size: Size<u32>,
    payload: Arc<dyn Any + Send + Sync>,
}

impl ExternalTexture {
    /// Wraps a renderer-private texture payload. `size` is in pixels.
    pub fn new(size: Size<u32>, payload: Arc<dyn Any + Send + Sync>) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self {
            id: ExternalTextureId(NEXT_ID.fetch_add(1, Ordering::SeqCst)),
            size,
            payload,
        }
    }

    /// The texture's stable id.
    pub fn id(&self) -> ExternalTextureId {
        self.id
    }

    /// The texture's size in pixels.
    pub fn size(&self) -> Size<u32> {
        self.size
    }

    /// The renderer-private payload.
    pub fn payload(&self) -> &Arc<dyn Any + Send + Sync> {
        &self.payload
    }

    /// The payload downcast to the renderer's own texture type.
    pub fn downcast<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.payload.downcast_ref::<T>()
    }
}

impl fmt::Debug for ExternalTexture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExternalTexture")
            .field("id", &self.id)
            .field("size", &self.size)
            .finish()
    }
}

impl PartialEq for ExternalTexture {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ExternalTexture {}

/// What the platform can import from dma-bufs, if anything (Linux).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmaBufCapabilities {
    /// The DRM render node as `(major, minor)`.
    pub device: (u32, u32),
    /// Importable `(DRM fourcc, modifier)` pairs.
    pub formats: Vec<(u32, u64)>,
}

/// One plane of a dma-buf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaBufPlane {
    /// The plane's file descriptor (`i32` so the type exists on every target).
    pub fd: i32,
    /// Offset into the plane, in bytes.
    pub offset: u32,
    /// Row stride, in bytes.
    pub stride: u32,
}

/// A dma-buf to import as a GPU texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmaBufDescriptor {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The DRM fourcc of the buffer.
    pub fourcc: u32,
    /// The DRM format modifier.
    pub modifier: u64,
    /// The buffer's planes.
    pub planes: Vec<DmaBufPlane>,
}
