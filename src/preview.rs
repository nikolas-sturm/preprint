use std::sync::Arc;

use gpui::{
    Bounds, Corners, IntoElement, Point, RenderImage, Styled, canvas, fill, hsla, outline, px, size,
};
use image::{DynamicImage, Frame, ImageBuffer, Rgba};

use crate::processing::CropRect;

#[derive(Clone)]
pub(crate) struct PreviewBitmap {
    pub(crate) render: Arc<RenderImage>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl PreviewBitmap {
    pub(crate) fn from_dynamic(image: &DynamicImage) -> Self {
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        let mut bgra = rgba.into_raw();
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let buffer = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, bgra)
            .expect("RGBA preview dimensions must match buffer");
        Self {
            render: Arc::new(RenderImage::new(vec![Frame::new(buffer)])),
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

pub fn zoomed_fit_image(container: (f32, f32), image: (u32, u32), zoom: f32) -> FitRect {
    let base = fit_image(container, image);
    let zoom = zoom.max(0.0);
    let width = base.width * zoom;
    let height = base.height * zoom;
    FitRect {
        x: (container.0 - width) * 0.5,
        y: (container.1 - height) * 0.5,
        width,
        height,
    }
}

pub(crate) fn print_preview_canvas(
    image: PreviewBitmap,
    crop_rect: Option<CropRect>,
    zoom: f32,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let fit = zoomed_fit_image(
                (f32::from(bounds.size.width), f32::from(bounds.size.height)),
                (image.width, image.height),
                zoom,
            );
            let image_bounds = Bounds::new(
                Point::new(bounds.origin.x + px(fit.x), bounds.origin.y + px(fit.y)),
                size(px(fit.width), px(fit.height)),
            );
            let _ = window.paint_image(
                image_bounds,
                Corners::default(),
                image.render.clone(),
                0,
                false,
            );
            if let Some(crop) = crop_rect {
                let scale_x = fit.width / image.width as f32;
                let scale_y = fit.height / image.height as f32;
                let image_x = f32::from(image_bounds.origin.x);
                let image_y = f32::from(image_bounds.origin.y);
                let crop_x = image_x + crop.x as f32 * scale_x;
                let crop_y = image_y + crop.y as f32 * scale_y;
                let crop_width = crop.width as f32 * scale_x;
                let crop_height = crop.height as f32 * scale_y;
                let mask = hsla(0.0, 0.0, 0.0, 0.62);
                window.paint_quad(fill(
                    Bounds::new(
                        Point::new(px(image_x), px(image_y)),
                        size(px(fit.width), px(crop_y - image_y)),
                    ),
                    mask,
                ));
                window.paint_quad(fill(
                    Bounds::new(
                        Point::new(px(image_x), px(crop_y + crop_height)),
                        size(
                            px(fit.width),
                            px(image_y + fit.height - crop_y - crop_height),
                        ),
                    ),
                    mask,
                ));
                window.paint_quad(fill(
                    Bounds::new(
                        Point::new(px(image_x), px(crop_y)),
                        size(px(crop_x - image_x), px(crop_height)),
                    ),
                    mask,
                ));
                window.paint_quad(fill(
                    Bounds::new(
                        Point::new(px(crop_x + crop_width), px(crop_y)),
                        size(
                            px(image_x + fit.width - crop_x - crop_width),
                            px(crop_height),
                        ),
                    ),
                    mask,
                ));
                window.paint_quad(outline(
                    Bounds::new(
                        Point::new(px(crop_x), px(crop_y)),
                        size(px(crop_width), px(crop_height)),
                    ),
                    hsla(0.0, 0.0, 1.0, 0.95),
                    gpui::BorderStyle::Solid,
                ));
            }
        },
    )
    .size_full()
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
    fn zoom_scales_fitted_image_around_center() {
        assert_eq!(
            zoomed_fit_image((200.0, 100.0), (400, 400), 2.0),
            FitRect {
                x: 0.0,
                y: -50.0,
                width: 200.0,
                height: 200.0,
            }
        );
    }
}
