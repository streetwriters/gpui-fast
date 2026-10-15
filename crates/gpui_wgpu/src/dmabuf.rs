//! dma-buf support for the Vulkan backend (Linux/FreeBSD).
//!
//! Two halves:
//!
//! - [`capabilities`] answers "which (DRM fourcc, modifier) pairs can this
//!   adapter sample?", which the nested compositor turns into its
//!   `linux-dmabuf` feedback. The list must be exact: whatever it advertises,
//!   a client may send back.
//! - [`import`] wraps a dma-buf's planes into a `wgpu::Texture` without
//!   copying. The result is the same [`SurfaceTexture`](crate::SurfaceTexture)
//!   the synthetic upload path produces, so the renderer samples both
//!   identically.
//!
//! The raw Vulkan calls go through `ash`, the same version wgpu-hal uses.
//! wgpu only loads Vulkan at runtime, so `ash`'s default `loaded` feature is
//! the right one; there is no build-time link to libvulkan.
//!
//! Colour space: the renderer's target and sprite atlas are non-sRGB
//! (`Bgra8Unorm`), so the imported image is sampled raw. The Vulkan format is
//! chosen to match the DRM fourcc's byte order exactly (ARGB8888 → BGRA bytes →
//! `B8G8R8A8_UNORM` + [`wgpu::TextureFormat::Bgra8Unorm`]).
//!
//! Synchronization is left to the driver's implicit dma-fence handling (the
//! first cut; explicit sync is a follow-up if the machine shows tearing).

use std::os::fd::{AsRawFd as _, BorrowedFd, IntoRawFd as _, OwnedFd, RawFd};
use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail};
use ash::vk;
use gpui::{DmaBufCapabilities, DmaBufDescriptor, ExternalTexture};
use wgpu::hal::{self, api::Vulkan};

use crate::SurfaceTexture;

/// DRM fourccs we can import, with the Vulkan format whose memory layout they
/// match and the wgpu format the texture is created with. WebKit's ARGB/XRGB
/// buffers are what the nested compositor sees in practice; the ABGR/XBGR pair
/// is here because a client may pick it.
const FORMATS: &[(u32, vk::Format, wgpu::TextureFormat)] = &[
    (
        fourcc(b'A', b'R', b'2', b'4'),
        vk::Format::B8G8R8A8_UNORM,
        wgpu::TextureFormat::Bgra8Unorm,
    ),
    (
        fourcc(b'X', b'R', b'2', b'4'),
        vk::Format::B8G8R8A8_UNORM,
        wgpu::TextureFormat::Bgra8Unorm,
    ),
    (
        fourcc(b'A', b'B', b'2', b'4'),
        vk::Format::R8G8B8A8_UNORM,
        wgpu::TextureFormat::Rgba8Unorm,
    ),
    (
        fourcc(b'X', b'B', b'2', b'4'),
        vk::Format::R8G8B8A8_UNORM,
        wgpu::TextureFormat::Rgba8Unorm,
    ),
];

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

/// The (fourcc, Vulkan format, wgpu format) triple for a fourcc, if known.
fn format_for_fourcc(fourcc: u32) -> Option<(vk::Format, wgpu::TextureFormat)> {
    FORMATS
        .iter()
        .find(|(code, _, _)| *code == fourcc)
        .map(|(_, vulkan, wgpu)| (*vulkan, *wgpu))
}

/// Enumerates the (fourcc, modifier) pairs `adapter` can import, along with its
/// DRM render (or primary) node. Returns `None` when the adapter is not Vulkan
/// or the driver lacks the dma-buf external-memory extensions.
pub fn capabilities(adapter: &wgpu::Adapter) -> Option<DmaBufCapabilities> {
    // SAFETY: `as_hal` is sound for any adapter; the borrow only lives for this
    // function.
    let hal_adapter = unsafe { adapter.as_hal::<Vulkan>() }?;
    let shared = hal_adapter.shared_instance();
    if shared.instance_api_version() < vk::API_VERSION_1_1 {
        // Imported fds need the Vulkan 1.1 external-memory core (or the KHR
        // extensions, which wgpu only enables on the 1.1 path).
        return None;
    }
    let instance = shared.raw_instance();
    let physical_device = hal_adapter.raw_physical_device();
    // SAFETY: the instance and physical device are valid for the life of the
    // adapter, and the queries below only read properties.
    unsafe { query_capabilities(instance, physical_device) }
}

unsafe fn query_capabilities(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Option<DmaBufCapabilities> {
    if !has_extension(
        instance,
        physical_device,
        ash::ext::external_memory_dma_buf::NAME,
    ) || !has_extension(
        instance,
        physical_device,
        ash::ext::image_drm_format_modifier::NAME,
    ) {
        return None;
    }

    let device = unsafe { drm_node(instance, physical_device) }?;

    let mut formats = Vec::new();
    for (fourcc, vulkan_format, _) in FORMATS {
        let modifiers = unsafe { format_modifiers(instance, physical_device, *vulkan_format) };
        for modifier in modifiers {
            if unsafe { can_import(instance, physical_device, *vulkan_format, modifier) } {
                formats.push((*fourcc, modifier));
            }
        }
    }
    if formats.is_empty() {
        return None;
    }
    Some(DmaBufCapabilities { device, formats })
}

/// The `(major, minor)` of the DRM render node, falling back to the primary
/// node for drivers that report no render node.
unsafe fn drm_node(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Option<(u32, u32)> {
    let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
    // SAFETY: the chain points at a live struct for the duration of the call.
    unsafe { instance.get_physical_device_properties2(physical_device, &mut properties) };
    if drm.has_render == vk::TRUE {
        Some((drm.render_major as u32, drm.render_minor as u32))
    } else if drm.has_primary == vk::TRUE {
        Some((drm.primary_major as u32, drm.primary_minor as u32))
    } else {
        None
    }
}

fn has_extension(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    name: &std::ffi::CStr,
) -> bool {
    // SAFETY: `enumerate_device_extension_properties` is valid on any physical
    // device of this instance.
    match unsafe { instance.enumerate_device_extension_properties(physical_device) } {
        Ok(properties) => properties.iter().any(|property| {
            property
                .extension_name_as_c_str()
                .is_ok_and(|extension| extension == name)
        }),
        Err(_) => false,
    }
}

/// All DRM modifiers `format` supports that can also be sampled with linear
/// filtering (which the renderer's sampler needs for video scaling) **and need
/// a single plane**.
///
/// The plane count comes from `VkDrmFormatModifierPropertiesEXT` and is the
/// important half: Intel's `I915_FORMAT_MOD_Y_TILED_CCS` is an ARGB8888
/// modifier with *two* planes (the surface plus its compression aux buffer),
/// and GBM happily allocates it. Advertising it would make WebKitGTK send a
/// two-plane buffer the single-plane import cannot take — and the `create_immed`
/// rejection then kills the client connection (there is no other way to report
/// a failed `create_immed` over `linux-dmabuf`). Linear/X/Y/Yf tiling are the
/// single-plane survivors.
unsafe fn format_modifiers(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    format: vk::Format,
) -> Vec<u64> {
    let mut count_list = vk::DrmFormatModifierPropertiesListEXT::default();
    let mut properties = vk::FormatProperties2::default().push_next(&mut count_list);
    // SAFETY: the chain points at live structs for the duration of the call.
    unsafe {
        instance.get_physical_device_format_properties2(physical_device, format, &mut properties)
    };
    let count = count_list.drm_format_modifier_count as usize;
    if count == 0 {
        return Vec::new();
    }

    let mut modifiers = vec![vk::DrmFormatModifierPropertiesEXT::default(); count];
    let mut list = vk::DrmFormatModifierPropertiesListEXT {
        drm_format_modifier_count: count as u32,
        p_drm_format_modifier_properties: modifiers.as_mut_ptr(),
        ..Default::default()
    };
    let mut properties = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: the list points at an allocation the driver may fill; the count
    // is what the first call reported.
    unsafe {
        instance.get_physical_device_format_properties2(physical_device, format, &mut properties)
    };
    modifiers.truncate(list.drm_format_modifier_count as usize);

    modifiers
        .into_iter()
        .filter(|modifier| modifier.drm_format_modifier_plane_count == 1)
        .filter(|modifier| {
            modifier
                .drm_format_modifier_tiling_features
                .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR)
        })
        .map(|modifier| modifier.drm_format_modifier)
        .collect()
}

/// Whether the driver says it can import an image of `format` with `modifier`
/// from a dma-buf. This is the check the feedback's list is built from; it
/// matches what Chromium's Dawn and Mutter ask (success plus
/// `IMPORTABLE_BIT`).
unsafe fn can_import(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    format: vk::Format,
    modifier: u64,
) -> bool {
    // Both structures extend `PhysicalDeviceImageFormatInfo2` directly; ash's
    // `push_next` links them in this order (external first, then modifier).
    let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let mut drm = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
        .drm_format_modifier(modifier)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(format)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .push_next(&mut external)
        .push_next(&mut drm);

    let mut external_properties = vk::ExternalImageFormatProperties::default();
    let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external_properties);
    // SAFETY: the chain points at live structs for the duration of the call.
    let result = unsafe {
        instance.get_physical_device_image_format_properties2(
            physical_device,
            &info,
            &mut properties,
        )
    };
    if result.is_err() {
        return false;
    }
    external_properties
        .external_memory_properties
        .external_memory_features
        .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
}

/// Checks that the driver will create an image with `fourcc` and `modifier` at
/// all. Used by `dmabuf-probe`: it isolates "the extension/format is not
/// enabled" from "the import failed", which are easy to confuse.
pub fn can_create_image(device: &wgpu::Device, fourcc: u32, modifier: u64) -> anyhow::Result<()> {
    // SAFETY: `as_hal` is sound for any device.
    let hal_device =
        unsafe { device.as_hal::<Vulkan>() }.context("dmabuf-probe: not a Vulkan device")?;
    let raw_device = hal_device.raw_device();
    let (vulkan_format, _) =
        format_for_fourcc(fourcc).ok_or_else(|| anyhow!("unsupported fourcc {fourcc:#x}"))?;

    let modifiers = [modifier];
    let mut list =
        vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&modifiers);
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vulkan_format)
        .extent(vk::Extent3D {
            width: 4,
            height: 4,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .push_next(&mut list)
        .push_next(&mut external);

    // SAFETY: the chain points at live structs for the duration of the call;
    // the image is destroyed right after.
    let image = unsafe { raw_device.create_image(&info, None) }
        .map_err(|error| anyhow!("vkCreateImage with modifier {modifier:#x}: {error}"))?;
    // SAFETY: `image` was just created on `raw_device` and is not bound.
    unsafe { raw_device.destroy_image(image, None) };
    Ok(())
}

/// Imports a dma-buf as a GPU texture the renderer can sample.
///
/// The planes' file descriptors are duplicated; the duplicates are closed when
/// wgpu destroys the texture, so the caller may drop its own dma-buf handles as
/// soon as this returns.
pub fn import(
    device: &wgpu::Device,
    descriptor: &DmaBufDescriptor,
) -> anyhow::Result<ExternalTexture> {
    // SAFETY: `as_hal` is sound for any device.
    let hal_device = unsafe { device.as_hal::<Vulkan>() }
        .context("dma-buf import requires the Vulkan backend")?;
    // Without the extension enabled the image is not laid out by the
    // modifier, and the import reads sheared pixels: refuse instead, so the
    // caller sees an error rather than a wrong texture.
    // `WgpuContext::create_device` enables it whenever `capabilities`
    // advertises formats.
    if !hal_device
        .enabled_device_extensions()
        .contains(&ash::ext::image_drm_format_modifier::NAME)
    {
        bail!("the device was opened without VK_EXT_image_drm_format_modifier");
    }
    let raw_device = hal_device.raw_device().clone();
    let (vulkan_format, wgpu_format) = format_for_fourcc(descriptor.fourcc)
        .ok_or_else(|| anyhow!("unsupported dma-buf fourcc {:#x}", descriptor.fourcc))?;
    let [plane] = descriptor.planes.as_slice() else {
        bail!(
            "expected a single-plane dma-buf, got {} planes",
            descriptor.planes.len()
        );
    };
    let width = descriptor.width.max(1);
    let height = descriptor.height.max(1);

    let fd = dup_cloexec(plane.fd).context("duplicating the dma-buf plane fd")?;

    // A DRM-modifier image whose layout the *exporter* chose: the plane
    // offsets/strides come from the dma-buf, not from the driver. (The client's
    // pitch is authoritative: GBM reports the linear pitch for tiled buffers
    // and aligning it up to 512 was measured to *break* the layout on ANV.)
    let plane_layout = vk::SubresourceLayout {
        offset: u64::from(plane.offset),
        size: 0,
        row_pitch: u64::from(plane.stride),
        array_pitch: 0,
        depth_pitch: 0,
    };
    let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(descriptor.modifier)
        .plane_layouts(std::slice::from_ref(&plane_layout));
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vulkan_format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .push_next(&mut explicit)
        .push_next(&mut external);
    // SAFETY: the chain points at live structs for the duration of the call.
    let image = ImageGuard::new(
        raw_device.clone(),
        unsafe { raw_device.create_image(&image_info, None) }
            .with_context(|| format!("creating a {width}x{height} image for the dma-buf"))?,
    );

    // The memory type is the driver's choice among those compatible with the
    // image. `memory_type_bits` is only an upper bound: on some drivers the fd
    // is not importable with every advertised type (`vkGetMemoryFdPropertiesKHR`
    // is the authoritative answer, but the instance handle needed to load it is
    // not available here), so try each advertised type until one imports.
    // SAFETY: `image` is valid and not bound.
    let requirements = unsafe { raw_device.get_image_memory_requirements(image.raw()) };
    if requirements.memory_type_bits == 0 {
        bail!("no compatible memory type for the imported image");
    }

    let mut memory = None;
    let mut last_error = None;
    for memory_type_index in 0..32u32 {
        if requirements.memory_type_bits & (1 << memory_type_index) == 0 {
            continue;
        }
        // Both structs are created *inside* the pass: `push_next` splices the
        // new allocation into the front of whatever chain the pushed struct
        // already carries. Re-attaching the same instances on a second pass
        // would splice the chain into itself (ash 0.38 moves the old chain to
        // the tail), and a driver walking it then loops forever. Fresh
        // instances cannot carry a chain.
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image.raw());
        let mut import_info = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(fd.as_raw_fd());
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size.max(1))
            .memory_type_index(memory_type_index)
            .push_next(&mut dedicated)
            .push_next(&mut import_info);
        // SAFETY: the chain is live and `fd` stays open across the call (a
        // failed allocation leaves ownership with us).
        match unsafe { raw_device.allocate_memory(&allocate_info, None) } {
            Ok(raw) => {
                memory = Some(MemoryGuard::new(raw_device.clone(), raw));
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let memory = memory.ok_or_else(|| anyhow!("importing the dma-buf memory: {last_error:?}"))?;
    // Ownership of the fd moves to the driver with that successful call: it
    // closes the descriptor when the memory is freed (ANV closes it *inside*
    // `vkAllocateMemory` — measured). Forgetting it here is not an optimisation
    // but a correctness fix: closing it ourselves can take out a descriptor
    // number that has since been reused — the nested compositor dup's plane fds
    // into fresh `OwnedFd`s constantly — which is what Rust's IO-safety check
    // aborts on ("owned file descriptor already closed"). `OwnedFd` still closes
    // it on every error path above this line.
    let _driver_owned: RawFd = fd.into_raw_fd();

    // SAFETY: both handles were created on this device; the offset is 0, as
    // required for a DRM-modifier image.
    unsafe { raw_device.bind_image_memory(image.raw(), memory.raw(), 0) }
        .context("binding the imported memory to the image")?;

    let hal_desc = hal::TextureDescriptor {
        label: Some("dmabuf_surface"),
        size: wgt_extent(width, height),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu_format,
        usage: wgpu::wgt::TextureUses::RESOURCE,
        memory_flags: hal::MemoryFlags::empty(),
        view_formats: Vec::new(),
    };
    let image_raw = image.raw();
    // The drop callback owns the image and the memory (freeing the memory
    // closes the fd): wgpu calls it when the last use of the texture has retired.
    let drop_callback: hal::DropCallback = Box::new(move || {
        drop((image, memory));
    });
    // SAFETY: the image was created per `hal_desc` (format, size, usage) and
    // its memory stays alive until the callback runs.
    let hal_texture = unsafe {
        hal_device.texture_from_raw(
            image_raw,
            &hal_desc,
            Some(drop_callback),
            hal::vulkan::TextureMemory::External,
        )
    };
    // SAFETY: `hal_texture` wraps the image passed above, with a matching
    // descriptor; ownership moves into wgpu.
    let texture = unsafe {
        device.create_texture_from_hal::<Vulkan>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("dmabuf_surface"),
                size: wgt_extent(width, height),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu_format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        )
    };

    Ok(ExternalTexture::new(
        gpui::Size { width, height },
        Arc::new(SurfaceTexture {
            texture,
            size: (width, height),
        }),
    ))
}

fn wgt_extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

/// Duplicates the client's plane fd (close-on-exec, at or above 3). The
/// duplicate is closed on drop until a successful `vkAllocateMemory` hands it to
/// the driver (`into_raw_fd` in [`import`]).
fn dup_cloexec(fd: RawFd) -> std::io::Result<OwnedFd> {
    if fd < 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid dma-buf fd",
        ));
    }
    // SAFETY: the descriptor belongs to the caller's `DmaBufDescriptor`, which
    // keeps it open for the duration of `import`.
    unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()
}

/// Destroys a Vulkan image when it (or the import that owns it) is dropped.
struct ImageGuard {
    device: ash::Device,
    image: vk::Image,
}

impl ImageGuard {
    fn new(device: ash::Device, image: vk::Image) -> Self {
        Self { device, image }
    }

    fn raw(&self) -> vk::Image {
        self.image
    }
}

impl Drop for ImageGuard {
    fn drop(&mut self) {
        // SAFETY: the image belongs to this device and is not in use anymore
        // (the caller only drops it once rendering has retired).
        unsafe { self.device.destroy_image(self.image, None) };
    }
}

/// Frees imported memory when it (or the import that owns it) is dropped.
struct MemoryGuard {
    device: ash::Device,
    memory: vk::DeviceMemory,
}

impl MemoryGuard {
    fn new(device: ash::Device, memory: vk::DeviceMemory) -> Self {
        Self { device, memory }
    }

    fn raw(&self) -> vk::DeviceMemory {
        self.memory
    }
}

impl Drop for MemoryGuard {
    fn drop(&mut self) {
        // SAFETY: the memory belongs to this device and is not in use anymore.
        unsafe { self.device.free_memory(self.memory, None) };
    }
}
