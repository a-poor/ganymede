//! PlanetRise animation: cached terrain and sphere geometry, one texture per frame.
use crate::theme::ActiveTheme;
use gpui::{
    Context, Hsla, ObjectFit, Render, RenderImage, StyledImage, Task, Window, div, img, prelude::*,
};
use image::{Frame, RgbaImage};
use std::{
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

#[path = "planet_scene.rs"]
mod scene;
use scene::{SIDE, Scene};

// Generated from the same scene at build time; no runtime noise/texture generation.
const OPENING_MASK: &[u8; SIDE * SIDE] =
    include_bytes!(concat!(env!("OUT_DIR"), "/planet-opening.bin"));

const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 15);
type Palette = ([u8; 4], [u8; 4]);
static SCENE: OnceLock<Scene> = OnceLock::new();

pub(super) struct Planet {
    image: Option<Arc<RenderImage>>,
    palette: Option<Palette>,
    _task: Task<()>,
}
impl Planet {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| {
            if let Some(image) = this.image.take() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        let theme = cx.theme();
        let palette = (bgra(theme.launcher_art_fg), bgra(theme.launcher_art_bg));
        let opening = render_mask(OPENING_MASK, palette);
        let task = cx.spawn_in(window, async move |this, cx| {
            let mut time = 0.;
            let mut previous = Instant::now();
            loop {
                let started = Instant::now();
                let Ok((animate, palette, needs_image)) = this.update_in(cx, |this, window, cx| {
                    let theme = cx.theme();
                    let palette = (bgra(theme.launcher_art_fg), bgra(theme.launcher_art_bg));
                    (
                        should_animate(window.is_window_active(), cx.reduce_motion()),
                        palette,
                        this.image.is_none() || this.palette != Some(palette),
                    )
                }) else {
                    break;
                };
                // Reset the clock during pauses rather than jumping ahead on resume.
                let dt = started.duration_since(previous).as_secs_f64().min(0.1);
                previous = started;
                if animate {
                    time += dt;
                }
                if animate || needs_image {
                    let image = cx
                        .background_executor()
                        .spawn(async move {
                            render_mask(&SCENE.get_or_init(Scene::new).mask(time), palette)
                        })
                        .await;
                    if this
                        .update_in(cx, |this, window, cx| {
                            let theme = cx.theme();
                            let current =
                                (bgra(theme.launcher_art_fg), bgra(theme.launcher_art_bg));
                            // A palette change during rendering must not flash the old theme.
                            if current != palette {
                                return;
                            }
                            if let Some(old) = this.image.replace(image) {
                                let _ = window.drop_image(old);
                            }
                            this.palette = Some(palette);
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                let interval = if animate {
                    FRAME_INTERVAL
                } else {
                    Duration::from_millis(250)
                };
                cx.background_executor()
                    .timer(interval.saturating_sub(started.elapsed()))
                    .await;
            }
        });
        Self {
            image: Some(opening),
            palette: Some(palette),
            _task: task,
        }
    }
}
impl Render for Planet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .size_full()
            .bg(t.launcher_art_bg)
            .text_color(t.launcher_art_fg)
            .when_some(self.image.clone(), |v, image| {
                v.child(img(image).size_full().object_fit(ObjectFit::Cover))
            })
    }
}
fn should_animate(active: bool, reduced_motion: bool) -> bool {
    active && !reduced_motion
}
fn bgra(color: Hsla) -> [u8; 4] {
    let c = gpui::Rgba::from(color);
    [c.b, c.g, c.r, c.a].map(|v| (v * 255.).round() as u8)
}

fn render_mask(mask: &[u8], palette: Palette) -> Arc<RenderImage> {
    // Expand with nearest-neighbor sampling off-thread, preserving crisp dots.
    let width = SIDE * 3;
    let mut bytes = Vec::with_capacity(width * width * 4);
    for row in mask.as_chunks::<SIDE>().0 {
        let start = bytes.len();
        for &on in row {
            let pixel = if on != 0 { palette.0 } else { palette.1 };
            for _ in 0..3 {
                bytes.extend_from_slice(&pixel);
            }
        }
        let end = bytes.len();
        bytes.extend_from_within(start..end);
        bytes.extend_from_within(start..end);
    }
    Arc::new(RenderImage::new([Frame::new(
        RgbaImage::from_raw(width as u32, width as u32, bytes).unwrap(),
    )]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn animation_pauses_for_inactive_windows_and_reduced_motion() {
        assert!(should_animate(true, false));
        assert!(!should_animate(false, false));
        assert!(!should_animate(true, true));
        assert!(!should_animate(false, true));
    }
    #[test]
    fn embedded_opening_matches_live_animation() {
        let scene = Scene::new();
        assert_eq!(OPENING_MASK.as_slice(), scene.mask(0.));
        let start = Instant::now();
        let _ = render_mask(OPENING_MASK, ([0, 0, 0, 255], [255, 255, 255, 255]));
        eprintln!("Embedded opening frame: {:?}", start.elapsed());
    }
    #[test]
    fn frames_animate_without_changing_the_terrain() {
        let scene = SCENE.get_or_init(Scene::new);
        let first = scene.mask(0.);
        let later = scene.mask(2.);
        assert_eq!(first, scene.mask(0.));
        assert!(first.iter().zip(&later).filter(|(a, b)| a != b).count() > 100);
        let start = Instant::now();
        for i in 0..15 {
            let _ = render_mask(
                &scene.mask(i as f64 / 15.),
                ([0, 0, 0, 255], [255, 255, 255, 255]),
            );
        }
        eprintln!("15 frames: {:?}", start.elapsed());
        // A terrain pixel can only be brightened by foreground dust, never erased.
        for index in SIDE * 3 / 4 * SIDE..SIDE * SIDE {
            if scene.terrain()[index] != 0 {
                assert_eq!(first[index], 1);
                assert_eq!(later[index], 1);
            }
        }
    }
}
