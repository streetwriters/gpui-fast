//! Tests of `fast::image_scaling` against real windows: a big image on disk
//! is shown from a variant decoded at its display size, through `img` and
//! through an image cache, and comes back after the atlas loses it.

use crate::{
    AppContext as _, Bounds, Context, DevicePixels, Entity, ImageSource, IntoElement,
    ParentElement as _, Render, RenderImageParams, RetainAllImageCache, Styled as _,
    TestAppContext, VisualTestContext, Window, div, img, px,
};

fn big_image_on_disk(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "gpui-image-scaling-{}-{name}.png",
        std::process::id()
    ));
    let image = image::RgbaImage::from_fn(1600, 1200, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
    });
    image.save(&path).unwrap();
    path
}

struct Photo {
    source: ImageSource,
    cache: Option<Entity<RetainAllImageCache>>,
}

impl Render for Photo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut photo = img(self.source.clone()).size_full();
        if let Some(cache) = &self.cache {
            photo = photo.image_cache(cache);
        }
        div().w(px(200.)).h(px(150.)).child(photo)
    }
}

/// Draws what changed, as a frame that nothing refreshes does.
fn draw_changes(window: &mut VisualTestContext) {
    window.update(|window, cx| window.draw(cx).clear(cx));
}

fn draw(window: &mut VisualTestContext) {
    window.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
}

/// The tile bounds of the last sprite the window painted, if any.
fn painted_tile(window: &mut VisualTestContext) -> Option<Bounds<DevicePixels>> {
    window.update(|window, _| {
        window
            .painted_scene()
            .polychrome_sprites
            .last()
            .map(|sprite| sprite.tile.bounds)
    })
}

/// Draws and lets decodes finish until the window paints something.
fn settle(window: &mut VisualTestContext) {
    for _ in 0..4 {
        draw(window);
        window.run_until_parked();
    }
    draw(window);
}

#[gpui::test]
fn a_big_image_is_shown_from_a_variant_at_its_display_size(cx: &mut TestAppContext) {
    let path = big_image_on_disk("img");
    let source = ImageSource::from(path.clone());
    let (_, window) = cx.add_window_view(|_, _| Photo {
        source,
        cache: None,
    });
    let scale = window.update(|window, _| window.scale_factor());
    settle(window);

    let tile = painted_tile(window).expect("the variant is painted once decoded");
    let wanted = (200. * scale).ceil() as i32;
    assert!(
        tile.size.width.0 >= wanted && tile.size.width.0 < wanted + 64,
        "the variant is {:?}, wanted about {wanted} wide",
        tile.size
    );
    assert!(tile.size.width.0 < 1600);
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn the_image_comes_back_after_the_atlas_loses_it(cx: &mut TestAppContext) {
    let path = big_image_on_disk("miss");
    let source = ImageSource::from(path.clone());
    let (_, window) = cx.add_window_view(|_, _| Photo {
        source,
        cache: None,
    });
    settle(window);
    assert!(painted_tile(window).is_some());
    // Once the variant's pixels were handed out the atlas is all that holds
    // them; clear it.
    draw(window);
    window.update(|window, _| {
        for image_id in crate::fast::image_scaling::all_variant_ids() {
            window.sprite_atlas.remove(
                &RenderImageParams {
                    image_id,
                    frame_index: 0,
                }
                .into(),
            );
        }
    });
    draw(window);
    assert!(
        painted_tile(window).is_none(),
        "the lost image is not painted until decoded again"
    );
    settle(window);
    assert!(painted_tile(window).is_some(), "the image is decoded again");
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn an_image_cache_loads_big_images_lazily_too(cx: &mut TestAppContext) {
    let path = big_image_on_disk("cache");
    let source = ImageSource::from(path.clone());
    let cache: Entity<RetainAllImageCache> = cx.update(RetainAllImageCache::new);
    let (_, window) = cx.add_window_view({
        let cache = cache.clone();
        move |_, _| Photo {
            source,
            cache: Some(cache),
        }
    });
    settle(window);
    let tile = painted_tile(window).expect("painted through the cache");
    assert!(tile.size.width.0 < 1600);
    // Clearing the cache drops the source and, with it, its variants.
    window.update(|window, cx| cache.update(cx, |cache, cx| cache.clear(window, cx)));
    std::fs::remove_file(path).ok();
}

struct Gallery {
    photo: Entity<Photo>,
    other: Entity<Photo>,
    show_other: bool,
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let gallery = div().child(self.photo.clone());
        if self.show_other {
            gallery.child(self.other.clone())
        } else {
            gallery
        }
    }
}

/// The ids of the variants the window's atlas holds a tile for.
fn variants_in_atlas(window: &mut VisualTestContext) -> Vec<crate::ImageId> {
    window.update(|window, _| {
        crate::fast::image_scaling::all_variant_ids()
            .into_iter()
            .filter(|&image_id| {
                window
                    .sprite_atlas
                    .get_or_insert_with(
                        RenderImageParams {
                            image_id,
                            frame_index: 0,
                        }
                        .into(),
                        &mut || Ok(None),
                    )
                    .ok()
                    .flatten()
                    .is_some()
            })
            .collect()
    })
}

#[gpui::test]
fn a_variant_painted_by_a_reused_view_is_not_released_while_it_is_on_screen(
    cx: &mut TestAppContext,
) {
    let photo_path = big_image_on_disk("on-screen");
    let other_path = big_image_on_disk("on-screen-other");
    let photo = cx.new(|_| Photo {
        source: ImageSource::from(photo_path.clone()),
        cache: None,
    });
    let other = cx.new(|_| Photo {
        source: ImageSource::from(other_path.clone()),
        cache: None,
    });
    let (gallery, window) = cx.add_window_view({
        let (photo, other) = (photo.clone(), other.clone());
        move |_, _| Gallery {
            photo,
            other,
            show_other: false,
        }
    });
    settle(window);
    let shown = crate::fast::image_scaling::on_screen_ids();
    assert!(!shown.is_empty(), "the photo's variant is on screen");
    let tile = painted_tile(window).unwrap();

    // The window sits idle past the time an unused variant is released.
    std::thread::sleep(crate::fast::image_scaling::IDLE + std::time::Duration::from_millis(200));
    // Something else changes: another image paints, which is when idle
    // variants are looked for, while the photo's view is drawn from the last
    // frame without painting.
    window.update(|_, cx| {
        gallery.update(cx, |gallery, cx| {
            gallery.show_other = true;
            cx.notify();
        })
    });
    draw_changes(window);
    window.run_until_parked();
    draw_changes(window);

    let still = variants_in_atlas(window);
    for variant in &shown {
        assert!(
            still.contains(variant),
            "the variant {variant:?} was released while a sprite of the last frame points at it"
        );
    }
    assert_eq!(
        window.update(|window, _| window
            .painted_scene()
            .polychrome_sprites
            .iter()
            .any(|sprite| sprite.tile.bounds == tile)),
        true,
        "the photo is still painted"
    );
    std::fs::remove_file(photo_path).ok();
    std::fs::remove_file(other_path).ok();
}

struct Layered {
    source: ImageSource,
    scroll: crate::ScrollHandle,
}

impl Render for Layered {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use crate::{InteractiveElement as _, StatefulInteractiveElement as _};
        let layered = div()
            .id("layered")
            .w(px(200.))
            .h(px(150.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(img(self.source.clone()).w(px(200.)).h(px(150.)))
            .child(div().h(px(3000.)));
        div().size_full().bg(crate::white()).child(layered)
    }
}

#[gpui::test]
fn a_variant_in_a_composited_layer_is_not_released_while_it_is_on_screen(cx: &mut TestAppContext) {
    use crate::fast::layers::policy::Decision;
    if !crate::fast::layers::COMPILED {
        return;
    }
    let photo_path = big_image_on_disk("layered");
    let other_path = big_image_on_disk("layered-other");
    // A second window, not drawn until the first has gone idle.
    let other = cx.add_window({
        let source = ImageSource::from(other_path.clone());
        move |_, _| Photo {
            source,
            cache: None,
        }
    });
    let (view, window) = cx.add_window_view({
        let source = ImageSource::from(photo_path.clone());
        move |_, _| Layered {
            source,
            scroll: crate::ScrollHandle::new(),
        }
    });
    // `update` draws the frame that follows it.
    let decide = |window: &mut VisualTestContext, decision| {
        window.update(|window, cx| {
            window.fast_layers.forced_decision = Some(decision);
            view.update(cx, |_, cx| cx.notify());
        });
    };
    settle(window);
    // The layer paints its content once, then only composites it.
    decide(window, Decision::Repaint);
    window.run_until_parked();
    decide(window, Decision::Repaint);
    let in_atlas = variants_in_atlas(window);
    let shown: Vec<_> = crate::fast::image_scaling::on_screen_ids()
        .into_iter()
        .filter(|variant| in_atlas.contains(variant))
        .collect();
    assert!(!shown.is_empty(), "the layer's image is on screen");
    decide(window, Decision::Composite);
    decide(window, Decision::Composite);
    let composited = window.update(|window, _| {
        window
            .layout_engine
            .as_ref()
            .unwrap()
            .retention
            .stats
            .layer_frames_composited
    });
    assert!(composited >= 2, "the layer was composited, {composited}");

    std::thread::sleep(crate::fast::image_scaling::IDLE + std::time::Duration::from_millis(200));
    // Another window paints an image, which is when idle variants are looked
    // for, while this one only composites the layer holding its own.
    for _ in 0..5 {
        window.update(|_, cx| {
            cx.update_window(other.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .ok();
        });
        window.run_until_parked();
    }
    decide(window, Decision::Composite);

    let still = variants_in_atlas(window);
    for variant in &shown {
        assert!(
            still.contains(variant),
            "the variant {variant:?} was released while a layer holds content that shows it"
        );
    }
    std::fs::remove_file(photo_path).ok();
    std::fs::remove_file(other_path).ok();
}
