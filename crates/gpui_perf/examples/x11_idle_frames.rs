#[cfg(target_os = "linux")]
mod linux {
    use gpui::*;
    use gpui_fast as gpui;
    use gpui_platform_fast as gpui_platform;
    use std::{
        process::Command,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    fn counters() -> (f64, u64) {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
        let cpu = |t: libc::timeval| t.tv_sec as f64 * 1e3 + t.tv_usec as f64 / 1e3;
        (
            cpu(usage.ru_utime) + cpu(usage.ru_stime),
            (usage.ru_nvcsw + usage.ru_nivcsw) as u64,
        )
    }

    #[derive(Default)]
    struct Probe {
        renders: AtomicUsize,
        animating: AtomicBool,
        key_down: Mutex<Option<Instant>>,
        latencies: Mutex<Vec<f64>>,
    }

    struct View {
        probe: Arc<Probe>,
        focus: FocusHandle,
        tick: u32,
    }

    impl Render for View {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.probe.renders.fetch_add(1, Ordering::Relaxed);
            if let Some(at) = self.probe.key_down.lock().unwrap().take() {
                self.probe
                    .latencies
                    .lock()
                    .unwrap()
                    .push(at.elapsed().as_secs_f64() * 1e3);
            }
            if self.probe.animating.load(Ordering::Relaxed) {
                self.tick = self.tick.wrapping_add(1);
                window.request_animation_frame();
            }
            div()
                .id("root")
                .track_focus(&self.focus)
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(0xe8e8e8))
                .bg(rgb(0x202124))
                .child(
                    div()
                        .w(px(40. + (self.tick % 200) as f32))
                        .h(px(40.))
                        .bg(rgb(0x65b879)),
                )
                .on_key_down(cx.listener(|this, _: &KeyDownEvent, _, cx| {
                    *this.probe.key_down.lock().unwrap() = Some(Instant::now());
                    cx.notify();
                }))
        }
    }

    pub fn run() {
        let output =
            std::path::PathBuf::from(std::env::args_os().nth(1).expect("output JSON path"));
        gpui_platform::application().run(move |cx: &mut App| {
            let probe = Arc::new(Probe::default());
            let view_probe = probe.clone();
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(100.), px(100.)),
                            size(px(1280.), px(800.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        let focus = cx.focus_handle();
                        window.focus(&focus, cx);
                        cx.new(|_| View {
                            probe: view_probe,
                            focus,
                            tick: 0,
                        })
                    },
                )
                .unwrap();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let before = counters();
                let before_renders = probe.renders.load(Ordering::Relaxed);
                let start = Instant::now();
                cx.background_executor()
                    .timer(Duration::from_secs(20))
                    .await;
                let elapsed = start.elapsed().as_secs_f64();
                let after = counters();
                let idle_renders = probe.renders.load(Ordering::Relaxed) - before_renders;

                let pid = std::process::id().to_string();
                for _ in 0..40 {
                    Command::new("sh")
                        .args([
                            "-c",
                            &format!(
                                "xdotool windowfocus $(xdotool search --pid {pid} --onlyvisible | head -1) key a"
                            ),
                        ])
                        .status()
                        .ok();
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                }

                probe.animating.store(true, Ordering::Relaxed);
                handle.update(cx, |_, _, cx| cx.notify()).unwrap();
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let anim_before = probe.renders.load(Ordering::Relaxed);
                let anim_start = Instant::now();
                cx.background_executor().timer(Duration::from_secs(5)).await;
                let anim_frames = probe.renders.load(Ordering::Relaxed) - anim_before;
                let anim_seconds = anim_start.elapsed().as_secs_f64();
                probe.animating.store(false, Ordering::Relaxed);

                let mut latencies = probe.latencies.lock().unwrap().clone();
                latencies.sort_by(|a, b| a.total_cmp(b));
                let result = serde_json::json!({
                    "elapsed_seconds": elapsed,
                    "process_cpu_ms": after.0 - before.0,
                    "context_switches": after.1 - before.1,
                    "idle_renders": idle_renders,
                    "animation_fps": anim_frames as f64 / anim_seconds,
                    "key_latency_ms": latencies,
                });
                std::fs::write(output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    linux::run();
    #[cfg(not(target_os = "linux"))]
    eprintln!("This benchmark requires Linux.");
}
