use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Bounds, Context, Corners, Entity, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Render, RenderImage,
    Styled, Subscription, Window, canvas, div, px, size,
};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Selectable,
    button::{Button, ButtonVariants as _},
    progress::Progress,
    slider::{Slider, SliderEvent, SliderState},
};
use image::{DynamicImage, Frame, ImageBuffer, Rgba};

use crate::{app::PreprintApp, i18n};

#[derive(Clone)]
pub(crate) struct PreviewBitmap {
    pub(crate) render: Arc<RenderImage>,
    pub(crate) rgba: Arc<[u8]>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl PreviewBitmap {
    pub(crate) fn from_dynamic(image: &DynamicImage) -> Self {
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        Self::from_rgba(width, height, rgba.into_raw())
    }

    fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        let mut bgra = rgba.clone();
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let buffer = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, bgra)
            .expect("RGBA preview dimensions must match buffer");
        let render = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
        Self {
            render,
            rgba: rgba.into(),
            width,
            height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub fn fit_image(container: (f32, f32), image: (u32, u32)) -> FitRect {
    let container_width = container.0.max(0.0);
    let container_height = container.1.max(0.0);
    if image.0 == 0 || image.1 == 0 || container_width == 0.0 || container_height == 0.0 {
        return FitRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
    }
    let scale = (container_width / image.0 as f32)
        .min(container_height / image.1 as f32)
        .min(1.0);
    let width = image.0 as f32 * scale;
    let height = image.1 as f32 * scale;
    FitRect {
        x: (container_width - width) * 0.5,
        y: (container_height - height) * 0.5,
        width,
        height,
    }
}

pub fn sample_lens_nearest(
    rgba: &[u8],
    width: u32,
    height: u32,
    center: (f32, f32),
    radius: u32,
    zoom: f32,
) -> Vec<u8> {
    let diameter = radius.saturating_mul(2);
    let mut output = vec![0; diameter as usize * diameter as usize * 4];
    if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
        return output;
    }
    let zoom = zoom.max(1.0);
    let radius_f = radius as f32;
    for y in 0..diameter {
        for x in 0..diameter {
            let dx = x as f32 + 0.5 - radius_f;
            let dy = y as f32 + 0.5 - radius_f;
            if dx * dx + dy * dy > radius_f * radius_f {
                continue;
            }
            let source_x = (center.0 + dx / zoom)
                .round()
                .clamp(0.0, width.saturating_sub(1) as f32) as u32;
            let source_y = (center.1 + dy / zoom)
                .round()
                .clamp(0.0, height.saturating_sub(1) as f32) as u32;
            let source = (source_y as usize * width as usize + source_x as usize) * 4;
            let destination = (y as usize * diameter as usize + x as usize) * 4;
            output[destination..destination + 4].copy_from_slice(&rgba[source..source + 4]);
        }
    }
    output
}

pub(crate) struct PreviewView {
    app: Entity<PreprintApp>,
    canvas_bounds: Bounds<Pixels>,
    dragging: bool,
    lens: Option<PreviewBitmap>,
    lens_center: Point<Pixels>,
    zoom: Entity<SliderState>,
    radius: Entity<SliderState>,
    _subscriptions: Vec<Subscription>,
}

impl PreviewView {
    pub(crate) fn new(
        app: Entity<PreprintApp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (initial_zoom, initial_radius) = {
            let app = app.read(cx);
            (app.preview.magnifier_zoom(), app.preview.magnifier_radius())
        };
        let zoom = cx.new(|_| {
            SliderState::new()
                .min(crate::app::MIN_MAGNIFIER_ZOOM)
                .max(crate::app::MAX_MAGNIFIER_ZOOM)
                .step(1.0)
                .default_value(initial_zoom)
        });
        let radius = cx.new(|_| {
            SliderState::new()
                .min(crate::app::MIN_MAGNIFIER_RADIUS)
                .max(crate::app::MAX_MAGNIFIER_RADIUS)
                .step(10.0)
                .default_value(initial_radius)
        });
        let observe = cx.observe_in(&app, window, |_, _, _, cx| cx.notify());
        let weak_app = app.downgrade();
        let preview_window = window.window_handle();
        let release = cx.on_release(move |_, cx| {
            let _ = weak_app.update(cx, |app, cx| {
                if app.preview_window == Some(preview_window) {
                    app.preview_window = None;
                    app.close_preview();
                    cx.notify();
                }
            });
        });
        let weak_app = app.downgrade();
        let zoom_subscription = cx.subscribe(&zoom, move |_, _, event: &SliderEvent, cx| {
            let SliderEvent::Change(value) = event;
            let _ = weak_app.update(cx, |app, cx| {
                app.preview.set_magnifier_zoom(value.start());
                cx.notify();
            });
        });
        let weak_app = app.downgrade();
        let radius_subscription = cx.subscribe(&radius, move |_, _, event: &SliderEvent, cx| {
            let SliderEvent::Change(value) = event;
            let _ = weak_app.update(cx, |app, cx| {
                app.preview.set_magnifier_radius(value.start());
                cx.notify();
            });
        });
        Self {
            app,
            canvas_bounds: Bounds::default(),
            dragging: false,
            lens: None,
            lens_center: Point::default(),
            zoom,
            radius,
            _subscriptions: vec![observe, release, zoom_subscription, radius_subscription],
        }
    }

    fn update_lens(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let snapshot = self.app.read(cx);
        if !snapshot.preview.magnifier_enabled() {
            self.lens = None;
            return;
        }
        let image = if snapshot.preview.softproof_enabled() {
            snapshot
                .preview_softproof
                .as_ref()
                .or(snapshot.preview_base.as_ref())
        } else {
            snapshot.preview_base.as_ref()
        };
        let Some(image) = image.cloned() else {
            return;
        };
        let fit = fit_image(
            (
                f32::from(self.canvas_bounds.size.width),
                f32::from(self.canvas_bounds.size.height),
            ),
            (image.width, image.height),
        );
        let local_x = f32::from(position.x) - f32::from(self.canvas_bounds.origin.x) - fit.x;
        let local_y = f32::from(position.y) - f32::from(self.canvas_bounds.origin.y) - fit.y;
        if fit.width == 0.0
            || fit.height == 0.0
            || local_x < 0.0
            || local_y < 0.0
            || local_x > fit.width
            || local_y > fit.height
        {
            self.lens = None;
            cx.notify();
            return;
        }
        let source = (
            local_x / fit.width * image.width as f32,
            local_y / fit.height * image.height as f32,
        );
        let radius = snapshot.preview.magnifier_radius().round() as u32;
        let pixels = sample_lens_nearest(
            &image.rgba,
            image.width,
            image.height,
            source,
            radius,
            snapshot.preview.magnifier_zoom(),
        );
        self.lens = Some(PreviewBitmap::from_rgba(radius * 2, radius * 2, pixels));
        self.lens_center = position;
        cx.notify();
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (state, image, softproof_available) = {
            let app = self.app.read(cx);
            let state = app.preview.clone();
            let image = if state.softproof_enabled() {
                app.preview_softproof.clone().or(app.preview_base.clone())
            } else {
                app.preview_base.clone()
            };
            (state, image, app.preview_softproof.is_some())
        };
        let image_available = image.is_some();

        let weak_app = self.app.downgrade();
        let softproof = Button::new("softproof")
            .icon(if state.softproof_enabled() {
                IconName::Eye
            } else {
                IconName::EyeOff
            })
            .label(if softproof_available {
                if state.softproof_enabled() {
                    i18n::translate("softproof-on")
                } else {
                    i18n::translate("softproof-off")
                }
            } else {
                i18n::translate("no-softproof-profile")
            })
            .disabled(!softproof_available)
            .on_click(move |_, _, cx| {
                let _ = weak_app.update(cx, |app, cx| {
                    app.preview
                        .set_softproof_enabled(!app.preview.softproof_enabled());
                    cx.notify();
                });
            });
        let weak_app = self.app.downgrade();
        let magnifier = Button::new("magnifier")
            .icon(IconName::Search)
            .label(i18n::translate("magnifier"))
            .selected(state.magnifier_enabled())
            .on_click(move |_, _, cx| {
                let _ = weak_app.update(cx, |app, cx| {
                    app.preview
                        .set_magnifier_enabled(!app.preview.magnifier_enabled());
                    cx.notify();
                });
            });
        let weak_app = self.app.downgrade();
        let is_fullscreen = window.is_fullscreen();
        let fullscreen = Button::new("fullscreen")
            .icon(if is_fullscreen {
                IconName::Minimize
            } else {
                IconName::Maximize
            })
            .label(if is_fullscreen {
                i18n::translate("exit-fullscreen")
            } else {
                i18n::translate("fullscreen")
            })
            .on_click(move |_, window, cx| {
                let fullscreen = !window.is_fullscreen();
                window.toggle_fullscreen();
                let _ = weak_app.update(cx, |app, cx| {
                    app.preview.fullscreen = fullscreen;
                    cx.notify();
                });
            });
        let weak_app = self.app.downgrade();
        let close = Button::new("close")
            .icon(IconName::Close)
            .label(i18n::translate("close"))
            .ghost()
            .on_click(move |_, window, cx| {
                let _ = weak_app.update(cx, |app, cx| {
                    app.close_preview();
                    cx.notify();
                });
                window.remove_window();
            });

        let mut toolbar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(softproof)
            .child(magnifier);
        if state.magnifier_enabled() {
            toolbar = toolbar
                .child(div().text_sm().child(format!(
                    "{} {:.0}x",
                    i18n::translate("zoom"),
                    state.magnifier_zoom()
                )))
                .child(div().w(px(140.)).child(Slider::new(&self.zoom)))
                .child(div().text_sm().child(format!(
                    "{} {:.0}px",
                    i18n::translate("lens"),
                    state.magnifier_radius()
                )))
                .child(div().w(px(140.)).child(Slider::new(&self.radius)));
        }
        toolbar = toolbar.child(div().flex_1()).child(fullscreen).child(close);

        let content: gpui::AnyElement = if state.rendering {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(Progress::new().w(px(300.)).value(state.progress() * 100.0))
                .child(state.progress_label().to_owned())
                .into_any_element()
        } else if let Some(image) = image {
            let lens = self.lens.clone();
            let lens_center = self.lens_center;
            let weak = cx.weak_entity();
            div()
                .id("preview-canvas")
                .flex_1()
                .m_3()
                .overflow_hidden()
                .child(
                    canvas(
                        move |bounds, _, cx| {
                            let _ = weak.update(cx, |view, _| view.canvas_bounds = bounds);
                        },
                        move |bounds, _, window, _| {
                            let fit = fit_image(
                                (f32::from(bounds.size.width), f32::from(bounds.size.height)),
                                (image.width, image.height),
                            );
                            let image_bounds = Bounds::new(
                                Point::new(
                                    bounds.origin.x + px(fit.x),
                                    bounds.origin.y + px(fit.y),
                                ),
                                size(px(fit.width), px(fit.height)),
                            );
                            let _ = window.paint_image(
                                image_bounds,
                                Corners::default(),
                                image.render.clone(),
                                0,
                                false,
                            );
                            if let Some(lens) = &lens {
                                let radius = lens.width as f32 * 0.5;
                                let lens_bounds = Bounds::new(
                                    Point::new(
                                        lens_center.x - px(radius),
                                        lens_center.y - px(radius),
                                    ),
                                    size(px(radius * 2.0), px(radius * 2.0)),
                                );
                                let _ = window.paint_image(
                                    lens_bounds,
                                    Corners::all(px(radius)),
                                    lens.render.clone(),
                                    0,
                                    false,
                                );
                            }
                        },
                    )
                    .size_full(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.dragging = true;
                        this.update_lens(event.position, cx);
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    if this.dragging {
                        this.update_lens(event.position, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.dragging = false;
                        this.lens = None;
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.dragging = false;
                        this.lens = None;
                        cx.notify();
                    }),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(i18n::translate("no-preview-yet"))
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(toolbar)
            .child(content)
            .when(
                state.magnifier_enabled() && image_available && !state.rendering,
                |view| {
                    view.child(
                        div()
                            .px_3()
                            .pb_2()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(i18n::translate("magnifier-hint")),
                    )
                },
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_centers_without_upscaling() {
        assert_eq!(
            fit_image((1000.0, 600.0), (400, 400)),
            FitRect {
                x: 300.0,
                y: 100.0,
                width: 400.0,
                height: 400.0
            }
        );
        assert_eq!(
            fit_image((200.0, 100.0), (400, 400)),
            FitRect {
                x: 50.0,
                y: 0.0,
                width: 100.0,
                height: 100.0
            }
        );
    }

    #[test]
    fn lens_uses_nearest_source_pixel_and_transparent_corners() {
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let lens = sample_lens_nearest(&rgba, 2, 2, (1.0, 1.0), 2, 4.0);
        assert_eq!(
            &lens[(2 * 4 + 2) * 4..(2 * 4 + 2) * 4 + 4],
            &[255, 255, 255, 255]
        );
        assert_eq!(&lens[0..4], &[0, 0, 0, 0]);
    }
}
