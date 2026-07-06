use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba, RgbaImage};
use imageproc::filter::gaussian_blur_f32;
use thiserror::Error;

const MM_PER_INCH: f32 = 25.4;

#[derive(Debug, Error)]
pub enum ProcessingError {
    #[error("print size must be positive")]
    InvalidPrintSize,
    #[error("image dimensions must be positive")]
    InvalidImageSize,
    #[error("border width must be zero or positive")]
    InvalidBorderWidth,
    #[error("output dimensions exceed supported size")]
    OutputTooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintSizeMm {
    pub width: f32,
    pub height: f32,
}

impl PrintSizeMm {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ppi {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BorderPixels {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderStyle {
    White,
    Black,
    MirroredBlur,
}

impl BorderStyle {
    pub fn label(self) -> String {
        let key = match self {
            Self::White => "border-white",
            Self::Black => "border-black",
            Self::MirroredBlur => "border-mirrored-blur",
        };
        egui_i18n::tr!(key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessingOptions {
    pub print_size: PrintSizeMm,
    pub border_mm: f32,
    pub border_style: BorderStyle,
}

impl ProcessingOptions {
    pub const fn new(print_size: PrintSizeMm, border_mm: f32, border_style: BorderStyle) -> Self {
        Self {
            print_size,
            border_mm,
            border_style,
        }
    }
}

pub fn calculate_ppi(
    pixel_width: u32,
    pixel_height: u32,
    print_size: PrintSizeMm,
) -> Result<Ppi, ProcessingError> {
    if pixel_width == 0 || pixel_height == 0 {
        return Err(ProcessingError::InvalidImageSize);
    }

    if !print_size.width.is_finite()
        || !print_size.height.is_finite()
        || print_size.width <= 0.0
        || print_size.height <= 0.0
    {
        return Err(ProcessingError::InvalidPrintSize);
    }

    Ok(Ppi {
        x: pixel_width as f32 / (print_size.width / MM_PER_INCH),
        y: pixel_height as f32 / (print_size.height / MM_PER_INCH),
    })
}

pub fn border_pixels(border_mm: f32, ppi: Ppi) -> Result<BorderPixels, ProcessingError> {
    if !border_mm.is_finite() || border_mm < 0.0 {
        return Err(ProcessingError::InvalidBorderWidth);
    }

    Ok(BorderPixels {
        x: ((border_mm / MM_PER_INCH) * ppi.x).round() as u32,
        y: ((border_mm / MM_PER_INCH) * ppi.y).round() as u32,
    })
}

pub fn add_border(
    source: &DynamicImage,
    options: &ProcessingOptions,
) -> Result<DynamicImage, ProcessingError> {
    let (width, height) = source.dimensions();
    let ppi = calculate_ppi(width, height, options.print_size)?;
    let border = border_pixels(options.border_mm, ppi)?;

    if is_16_bit_source(source) {
        return add_border_rgba16(source, options, border, width, height);
    }

    let output_width = width
        .checked_add(
            border
                .x
                .checked_mul(2)
                .ok_or(ProcessingError::OutputTooLarge)?,
        )
        .ok_or(ProcessingError::OutputTooLarge)?;
    let output_height = height
        .checked_add(
            border
                .y
                .checked_mul(2)
                .ok_or(ProcessingError::OutputTooLarge)?,
        )
        .ok_or(ProcessingError::OutputTooLarge)?;

    let source_rgba = source.to_rgba8();
    let mut output = match options.border_style {
        BorderStyle::White => RgbaImage::from_pixel(output_width, output_height, Rgba([255; 4])),
        BorderStyle::Black => {
            RgbaImage::from_pixel(output_width, output_height, Rgba([0, 0, 0, 255]))
        }
        BorderStyle::MirroredBlur => {
            blurred_edge_extension(&source_rgba, border, output_width, output_height)
        }
    };

    paste(&mut output, &source_rgba, border.x, border.y);
    Ok(DynamicImage::ImageRgba8(output))
}

fn add_border_rgba16(
    source: &DynamicImage,
    options: &ProcessingOptions,
    border: BorderPixels,
    width: u32,
    height: u32,
) -> Result<DynamicImage, ProcessingError> {
    let output_width = width
        .checked_add(
            border
                .x
                .checked_mul(2)
                .ok_or(ProcessingError::OutputTooLarge)?,
        )
        .ok_or(ProcessingError::OutputTooLarge)?;
    let output_height = height
        .checked_add(
            border
                .y
                .checked_mul(2)
                .ok_or(ProcessingError::OutputTooLarge)?,
        )
        .ok_or(ProcessingError::OutputTooLarge)?;

    let source_rgba = source.to_rgba16();
    let mut output = match options.border_style {
        BorderStyle::White => {
            ImageBuffer::from_pixel(output_width, output_height, Rgba([65535; 4]))
        }
        BorderStyle::Black => {
            ImageBuffer::from_pixel(output_width, output_height, Rgba([0, 0, 0, 65535]))
        }
        BorderStyle::MirroredBlur => {
            blurred_edge_extension16(&source_rgba, border, output_width, output_height)
        }
    };

    paste16(&mut output, &source_rgba, border.x, border.y);
    Ok(DynamicImage::ImageRgba16(output))
}

fn is_16_bit_source(source: &DynamicImage) -> bool {
    matches!(
        source,
        DynamicImage::ImageLuma16(_)
            | DynamicImage::ImageLumaA16(_)
            | DynamicImage::ImageRgb16(_)
            | DynamicImage::ImageRgba16(_)
    )
}

pub fn aspect_ratio_warning(pixel_width: u32, pixel_height: u32, print_size: PrintSizeMm) -> bool {
    if pixel_width == 0 || pixel_height == 0 || print_size.width <= 0.0 || print_size.height <= 0.0
    {
        return false;
    }

    let image_ratio = pixel_width as f32 / pixel_height as f32;
    let print_ratio = print_size.width / print_size.height;
    ((image_ratio - print_ratio).abs() / image_ratio) > 0.01
}

fn blurred_edge_extension(
    source: &RgbaImage,
    border: BorderPixels,
    output_width: u32,
    output_height: u32,
) -> RgbaImage {
    let source_width = source.width();
    let source_height = source.height();
    let mut extended = RgbaImage::new(output_width, output_height);

    for y in 0..output_height {
        for x in 0..output_width {
            let source_x = x.saturating_sub(border.x).min(source_width - 1);
            let source_y = y.saturating_sub(border.y).min(source_height - 1);
            extended.put_pixel(x, y, *source.get_pixel(source_x, source_y));
        }
    }

    let blurred = gaussian_blur_f32(&extended, border.x.max(border.y).max(1) as f32 * 0.5);
    let mut feathered = extended.clone();

    for y in 0..output_height {
        for x in 0..output_width {
            if x >= border.x
                && x < border.x + source_width
                && y >= border.y
                && y < border.y + source_height
            {
                continue;
            }

            let weight = feather_weight(x, y, border, source_width, source_height);
            let edge = extended.get_pixel(x, y);
            let blur = blurred.get_pixel(x, y);
            feathered.put_pixel(x, y, blend_rgba(*edge, *blur, weight));
        }
    }

    feathered
}

fn feather_weight(
    x: u32,
    y: u32,
    border: BorderPixels,
    source_width: u32,
    source_height: u32,
) -> f32 {
    let horizontal = if x < border.x {
        normalized_distance(border.x - x - 1, border.x)
    } else if x >= border.x + source_width {
        normalized_distance(x - border.x - source_width, border.x)
    } else {
        0.0
    };

    let vertical = if y < border.y {
        normalized_distance(border.y - y - 1, border.y)
    } else if y >= border.y + source_height {
        normalized_distance(y - border.y - source_height, border.y)
    } else {
        0.0
    };

    smoothstep(horizontal.max(vertical))
}

fn normalized_distance(distance_from_seam: u32, border_width: u32) -> f32 {
    if border_width <= 1 {
        return 0.0;
    }

    distance_from_seam as f32 / (border_width - 1) as f32
}

fn smoothstep(value: f32) -> f32 {
    let x = value.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn blend_rgba(edge: Rgba<u8>, blur: Rgba<u8>, weight: f32) -> Rgba<u8> {
    let mut out = [0; 4];
    for channel in 0..4 {
        let edge = edge.0[channel] as f32;
        let blur = blur.0[channel] as f32;
        out[channel] = (edge + (blur - edge) * weight).round().clamp(0.0, 255.0) as u8;
    }
    Rgba(out)
}

fn blurred_edge_extension16(
    source: &ImageBuffer<Rgba<u16>, Vec<u16>>,
    border: BorderPixels,
    output_width: u32,
    output_height: u32,
) -> ImageBuffer<Rgba<u16>, Vec<u16>> {
    let source_width = source.width();
    let source_height = source.height();
    let mut extended = ImageBuffer::new(output_width, output_height);

    for y in 0..output_height {
        for x in 0..output_width {
            let source_x = x.saturating_sub(border.x).min(source_width - 1);
            let source_y = y.saturating_sub(border.y).min(source_height - 1);
            extended.put_pixel(x, y, *source.get_pixel(source_x, source_y));
        }
    }

    let blurred = gaussian_blur_f32(&extended, border.x.max(border.y).max(1) as f32 * 0.5);
    let mut feathered = extended.clone();

    for y in 0..output_height {
        for x in 0..output_width {
            if x >= border.x
                && x < border.x + source_width
                && y >= border.y
                && y < border.y + source_height
            {
                continue;
            }

            let weight = feather_weight(x, y, border, source_width, source_height);
            let edge = extended.get_pixel(x, y);
            let blur = blurred.get_pixel(x, y);
            feathered.put_pixel(x, y, blend_rgba16(*edge, *blur, weight));
        }
    }

    feathered
}

fn blend_rgba16(edge: Rgba<u16>, blur: Rgba<u16>, weight: f32) -> Rgba<u16> {
    let mut out = [0; 4];
    for channel in 0..4 {
        let edge = edge.0[channel] as f32;
        let blur = blur.0[channel] as f32;
        out[channel] = (edge + (blur - edge) * weight).round().clamp(0.0, 65535.0) as u16;
    }
    Rgba(out)
}

fn paste(output: &mut RgbaImage, source: &RgbaImage, offset_x: u32, offset_y: u32) {
    for y in 0..source.height() {
        for x in 0..source.width() {
            output.put_pixel(offset_x + x, offset_y + y, *source.get_pixel(x, y));
        }
    }
}

fn paste16(
    output: &mut ImageBuffer<Rgba<u16>, Vec<u16>>,
    source: &ImageBuffer<Rgba<u16>, Vec<u16>>,
    offset_x: u32,
    offset_y: u32,
) {
    for y in 0..source.height() {
        for x in 0..source.width() {
            output.put_pixel(offset_x + x, offset_y + y, *source.get_pixel(x, y));
        }
    }
}
