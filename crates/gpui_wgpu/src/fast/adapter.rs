//! Choosing the first adapter to try at startup without initialising the
//! drivers of the others.
//!
//! Enumerating every backend loads each GPU's userspace driver, which on a
//! hybrid laptop wakes the discrete GPU even when the integrated one that
//! drives the display is chosen. When the user (`ZED_DEVICE_ID`) or the
//! compositor names an adapter, only the Vulkan adapters are enumerated and
//! the named one is tried; the full enumeration is left to the caller if that
//! fails.

use crate::{CompositorGpuHint, WgpuContext, WgpuSurfaceConfig};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};

/// How an adapter relates to the GPU the user or the compositor names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Named {
    ByUser,
    ByCompositor,
}

/// A device id of 0 is what backends that cannot tell GPUs apart report.
fn named(
    (vendor, device): (u32, u32),
    filter: Option<u32>,
    hint: Option<&CompositorGpuHint>,
) -> Option<Named> {
    if device == 0 {
        return None;
    }
    if filter == Some(device) {
        return Some(Named::ByUser);
    }
    hint.filter(|hint| hint.vendor_id == vendor && hint.device_id == device)
        .map(|_| Named::ByCompositor)
}

/// Tries the adapters named by `filter` and `hint`, the user's first, with
/// `try_one`, and returns the first result. The others are never touched.
pub(crate) fn named_first<A, R>(
    adapters: Vec<A>,
    ids: impl Fn(&A) -> (u32, u32),
    filter: Option<u32>,
    hint: Option<&CompositorGpuHint>,
    mut try_one: impl FnMut(A) -> Option<R>,
) -> Option<R> {
    let mut candidates: Vec<(Named, A)> = adapters
        .into_iter()
        .filter_map(|adapter| Some((named(ids(&adapter), filter, hint)?, adapter)))
        .collect();
    candidates.sort_by_key(|(named, _)| *named);
    candidates
        .into_iter()
        .find_map(|(_, adapter)| try_one(adapter))
}

#[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
/// The context and surface for a window whose adapter is named, or `None`
/// when nothing is named or the named adapter cannot be used.
pub(crate) fn select_named<W>(
    window: &W,
    raw_window_handle: RawWindowHandle,
    config: &WgpuSurfaceConfig,
    hint: Option<CompositorGpuHint>,
) -> Option<(WgpuContext, wgpu::Surface<'static>)>
where
    W: HasWindowHandle + HasDisplayHandle + std::fmt::Debug + Send + Sync + Clone + 'static,
{
    let filter = WgpuContext::device_id_filter();
    if filter.is_none() && hint.is_none() {
        return None;
    }
    let instance =
        WgpuContext::instance_for(wgpu::Backends::VULKAN, Some(Box::new(window.clone())));
    let surface = crate::wgpu_renderer::create_surface(&instance, raw_window_handle).ok()?;
    let adapters = gpui::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    let selected = named_first(
        adapters,
        |adapter| {
            let info = adapter.get_info();
            (info.vendor, info.device)
        },
        filter,
        hint.as_ref(),
        |adapter| {
            let info = adapter.get_info();
            log::info!(
                "Testing named adapter: {} ({:?})...",
                info.name,
                info.backend
            );
            match gpui::block_on(WgpuContext::try_adapter_with_surface(
                &adapter, &surface, config,
            )) {
                Ok(opened) => Some((adapter, opened)),
                Err(error) => {
                    log::info!("  Named adapter {} failed: {error:#}", info.name);
                    None
                }
            }
        },
    );
    let Some((adapter, (device, queue, dual_source_blending, color_texture_format))) = selected
    else {
        log::info!("No named adapter was usable; enumerating every adapter");
        return None;
    };
    let context = WgpuContext::from_native_adapter_and_device(
        instance,
        adapter,
        device,
        queue,
        dual_source_blending,
        color_texture_format,
    );
    Some((context, surface))
}

#[cfg(test)]
mod tests {
    use super::{CompositorGpuHint, named_first};

    const INTEL: (u32, u32) = (0x8086, 0x3e92);
    const NVIDIA: (u32, u32) = (0x10de, 0x2560);

    fn hint(ids: (u32, u32)) -> CompositorGpuHint {
        CompositorGpuHint {
            vendor_id: ids.0,
            device_id: ids.1,
        }
    }

    /// Adapters named by `ids`, recording which are tried; `usable` opens.
    fn run(
        adapters: &[(u32, u32)],
        filter: Option<u32>,
        hint: Option<&CompositorGpuHint>,
        usable: &[(u32, u32)],
    ) -> (Option<(u32, u32)>, Vec<(u32, u32)>) {
        let mut tried = Vec::new();
        let selected = named_first(
            adapters.to_vec(),
            |ids| *ids,
            filter,
            hint,
            |ids| {
                tried.push(ids);
                usable.contains(&ids).then_some(ids)
            },
        );
        (selected, tried)
    }

    #[test]
    fn a_matching_hint_is_tried_and_the_others_are_not() {
        let (selected, tried) = run(&[NVIDIA, INTEL], None, Some(&hint(INTEL)), &[INTEL, NVIDIA]);
        assert_eq!(selected, Some(INTEL));
        assert_eq!(tried, [INTEL]);
    }

    #[test]
    fn a_hint_that_matches_nothing_tries_nothing() {
        let (selected, tried) = run(
            &[NVIDIA, INTEL],
            None,
            Some(&hint((0x1002, 0x73bf))),
            &[INTEL],
        );
        assert_eq!(selected, None);
        assert!(tried.is_empty());
    }

    #[test]
    fn a_named_adapter_that_fails_the_test_leaves_the_rest_to_the_caller() {
        let (selected, tried) = run(&[NVIDIA, INTEL], None, Some(&hint(INTEL)), &[NVIDIA]);
        assert_eq!(selected, None);
        assert_eq!(tried, [INTEL]);
    }

    #[test]
    fn the_users_device_id_comes_before_the_compositors_hint() {
        let (selected, tried) = run(
            &[INTEL, NVIDIA],
            Some(NVIDIA.1),
            Some(&hint(INTEL)),
            &[INTEL, NVIDIA],
        );
        assert_eq!(selected, Some(NVIDIA));
        assert_eq!(tried, [NVIDIA]);
    }

    #[test]
    fn an_adapter_without_a_device_id_is_never_named() {
        let (selected, tried) = run(
            &[(0x8086, 0)],
            Some(0),
            Some(&hint((0x8086, 0))),
            &[(0x8086, 0)],
        );
        assert_eq!(selected, None);
        assert!(tried.is_empty());
    }
}
