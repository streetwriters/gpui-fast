//! Tests of `fast::image_scaling` against real windows: a big image on disk
//! is shown from a variant decoded at its display size, through `img` and
//! through an image cache, and comes back after the atlas loses it.

use crate::{
    Bounds, Context, DevicePixels, Entity, ImageSource, IntoElement, ParentElement as _, Render,
    RenderImageParams, RetainAllImageCache, Styled as _, TestAppContext, VisualTestContext, Window,
    div, img, px,
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
