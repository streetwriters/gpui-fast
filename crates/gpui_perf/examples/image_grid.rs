//! A screen of big photos shown small: 24 JPEGs of 4000x3000 at 200x150 points
//! in a grid under one shown at the size of the window. Prints the process
//! footprint, the Metal allocation, the time until every image is painted and
//! the frame intervals while scrolling the grid, as one JSON line.
//!
//! ```text
//! cargo run -p gpui_perf --release --example image_grid
//! ```

extern crate gpui_fast as gpui;
extern crate gpui_platform_fast as gpui_platform;

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The image_grid example measures Metal and is only available on macOS.");
}

#[cfg(target_os = "macos")]
mod mac {
    use gpui::{
        App, AppContext as _, Bounds, Context, InteractiveElement as _, IntoElement, ObjectFit,
        ParentElement as _, Render, ScrollHandle, StatefulInteractiveElement as _, Styled as _,
        StyledImage as _, Window, WindowBounds, WindowOptions, div, img, point, px, size,
    };
    use std::{
        cell::RefCell,
        path::PathBuf,
        rc::Rc,
        time::{Duration, Instant},
    };

    const PHOTOS: usize = 24;
    const COLUMNS: usize = 4;

    fn photos() -> Vec<PathBuf> {
        let dir = std::env::temp_dir().join("gpui-perf-photos");
        std::fs::create_dir_all(&dir).unwrap();
        (0..=PHOTOS)
            .map(|index| {
                let path = dir.join(format!("photo-{index}.jpg"));
                if !path.exists() {
                    let (width, height) = (4000u32, 3000u32);
                    let image = image::RgbImage::from_fn(width, height, |x, y| {
                        let noise = ((x.wrapping_mul(2654435761)
                            ^ y.wrapping_mul(40503)
                            ^ (index as u32 * 977))
                            >> 7)
                            & 31;
                        let wave =
                            (((x as f32 / 90.0).sin() + (y as f32 / 70.0).cos()) * 40.0) as i32;
                        image::Rgb([
                            ((x * 255 / width) as i32 + wave).clamp(0, 255) as u8,
                            ((y * 255 / height) as i32 + wave / 2 + noise as i32).clamp(0, 255)
                                as u8,
                            ((index * 10) as i32 + 100 + wave).clamp(0, 255) as u8,
                        ])
                    });
                    image.save(&path).unwrap();
                }
                path
            })
            .collect()
    }

    struct Grid {
        photos: Vec<PathBuf>,
        scroll: ScrollHandle,
        scrolling: bool,
    }

    impl Render for Grid {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            if std::env::var_os("GPUI_PERF_ONE").is_some() {
                return div()
                    .size_full()
                    .child(img(self.photos[0].clone()).w(px(200.)).h(px(150.)))
                    .into_any_element();
            }
            let banner = img(self.photos[PHOTOS].clone())
                .w(px(1400.))
                .h(px(900.))
                .object_fit(ObjectFit::Cover);
            let photos =
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(self.photos[..PHOTOS].iter().map(|path| {
                        img(path.clone())
                            .w(px(200.))
                            .h(px(150.))
                            .object_fit(ObjectFit::Cover)
                    }));
            if self.scrolling {
                div()
                    .id("grid")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(banner)
                    .child(photos.pb(px(2000.)))
                    .into_any_element()
            } else {
                // Every image is on screen at once: the banner behind the grid.
                div()
                    .id("grid")
                    .size_full()
                    .child(div().absolute().top_0().left_0().child(banner))
                    .child(div().absolute().top_0().left_0().w(px(1400.)).child(photos))
                    .into_any_element()
            }
        }
    }

    fn footprint_mb() -> f64 {
        let output = std::process::Command::new("footprint")
            .arg(std::process::id().to_string())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text
            .lines()
            .find(|line| line.contains("Footprint:"))
            .unwrap_or("");
        let mut words = line
            .split_whitespace()
            .skip_while(|word| *word != "Footprint:");
        let value: f64 = words.nth(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        match words.next() {
            Some("GB") => value * 1024.0,
            Some("KB") => value / 1024.0,
            _ => value,
        }
    }

    fn metal_allocated_mb() -> f64 {
        use objc::{msg_send, runtime::Object, sel, sel_impl};
        #[link(name = "Metal", kind = "framework")]
        unsafe extern "C" {
            fn MTLCreateSystemDefaultDevice() -> *mut Object;
        }
        unsafe {
            let device = MTLCreateSystemDefaultDevice();
            let bytes: u64 = msg_send![device, currentAllocatedSize];
            bytes as f64 / 1048576.0
        }
    }

    pub fn run() {
        let output = std::env::args_os().nth(1).map(PathBuf::from);
        let photos = photos();
        gpui_platform::application().run(move |cx: &mut App| {
            cx.activate(true);
            let started = Instant::now();
            let scroll = ScrollHandle::new();
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(100.), px(100.)),
                            size(px(1400.), px(900.)),
                        ))),
                        ..Default::default()
                    },
                    {
                        let scroll = scroll.clone();
                        move |_, cx| {
                            cx.new(|_| Grid {
                                photos,
                                scroll,
                                scrolling: false,
                            })
                        }
                    },
                )
                .unwrap();
            let intervals = Rc::new(RefCell::new(Vec::<f64>::new()));
            cx.spawn(async move |cx| {
                // Until the banner and all 24 photos are painted.
                let painted = loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(5))
                        .await;
                    let count = cx.update(|cx| {
                        handle
                            .update(cx, |_, window, _| {
                                window.painted_scene().polychrome_sprites.len()
                            })
                            .unwrap_or(0)
                    });
                    let expected = if std::env::var_os("GPUI_PERF_ONE").is_some() {
                        1
                    } else {
                        PHOTOS + 1
                    };
                    if count >= expected || started.elapsed() > Duration::from_secs(120) {
                        break started.elapsed();
                    }
                };
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let footprint = footprint_mb();
                let allocated = metal_allocated_mb();
                // Scroll the grid 6 points a frame and note when each frame ran.
                let times = Rc::new(RefCell::new(Vec::<Instant>::new()));
                cx.update(|cx| {
                    handle
                        .update(cx, |grid, window, cx| {
                            grid.scrolling = true;
                            cx.notify();
                            fn step(
                                window: &mut Window,
                                scroll: ScrollHandle,
                                times: Rc<RefCell<Vec<Instant>>>,
                                n: usize,
                            ) {
                                times.borrow_mut().push(Instant::now());
                                scroll.set_offset(point(px(0.), px(-(n as f32) * 6.)));
                                if n < 240 {
                                    window.refresh();
                                    window.on_next_frame(move |window, _| {
                                        step(window, scroll, times, n + 1)
                                    });
                                }
                            }
                            step(window, grid.scroll.clone(), times.clone(), 0);
                        })
                        .ok();
                });
                while times.borrow().len() < 241 {
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                }
                {
                    let times = times.borrow();
                    // The first frames lay the scroll container out; skip them.
                    for pair in times[10..].windows(2) {
                        intervals
                            .borrow_mut()
                            .push(pair[1].duration_since(pair[0]).as_secs_f64() * 1e3);
                    }
                }
                let mut sorted = intervals.borrow().clone();
                sorted.sort_by(f64::total_cmp);
                let result = serde_json::json!({
                    "first_frame_all_images_ms": painted.as_secs_f64() * 1e3,
                    "footprint_mb": footprint,
                    "metal_allocated_mb": allocated,
                    "scroll_interval_median_ms": sorted[sorted.len() / 2],
                    "scroll_interval_p95_ms": sorted[sorted.len() * 95 / 100],
                    "scroll_interval_max_ms": sorted[sorted.len() - 1],
                });
                match &output {
                    Some(path) => std::fs::write(path, result.to_string()).unwrap(),
                    None => println!("{result}"),
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
}

#[cfg(target_os = "macos")]
fn main() {
    mac::run();
}
