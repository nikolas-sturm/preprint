use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba, RgbaImage};
use imageproc::filter::gaussian_blur_f32;
use thiserror::Error;

use crate::loader::SourceBitDepth;

const MM_PER_INCH: f32 = 25.4;
pub const MAX_PROCESSING_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_CONCURRENT_PROCESSING_BYTES: u64 = MAX_PROCESSING_BYTES;

#[derive(Debug, Error)]
pub enum ProcessingError {
    #[error("processing cancelled")]
    Cancelled,
    #[error("print size must be positive")]
    InvalidPrintSize,
    #[error("image dimensions must be positive")]
    InvalidImageSize,
    #[error("border width must be zero or positive")]
    InvalidBorderWidth,
    #[error("output dimensions exceed supported size")]
    OutputTooLarge,
    #[error("processing would require approximately {estimated_mib} MiB; limit is {limit_mib} MiB")]
    ResourceLimit { estimated_mib: u64, limit_mib: u64 },
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
        crate::i18n::translate(key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessingOptions {
    pub print_size: PrintSizeMm,
    pub border_mm: f32,
    pub border_style: BorderStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessingRequirements {
    pub output_width: u32,
    pub output_height: u32,
    pub estimated_bytes: u64,
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
    add_border_with_cancel(source, options, || false)
}

pub fn add_border_with_cancel(
    source: &DynamicImage,
    options: &ProcessingOptions,
    cancelled: impl Fn() -> bool,
) -> Result<DynamicImage, ProcessingError> {
    add_border_with_blur(source, options, BlurMode::Exact, cancelled)
}

pub(crate) fn add_preview_border_with_cancel(
    source: &DynamicImage,
    options: &ProcessingOptions,
    cancelled: impl Fn() -> bool,
) -> Result<DynamicImage, ProcessingError> {
    add_border_with_blur(source, options, BlurMode::Fast, cancelled)
}

#[derive(Clone, Copy)]
enum BlurMode {
    Exact,
    Fast,
}

fn add_border_with_blur(
    source: &DynamicImage,
    options: &ProcessingOptions,
    blur_mode: BlurMode,
    cancelled: impl Fn() -> bool,
) -> Result<DynamicImage, ProcessingError> {
    check_cancelled(&cancelled)?;
    let (width, height) = source.dimensions();
    let source_bit_depth = SourceBitDepth::from_image(source);
    let (border, requirements) = processing_plan(width, height, source_bit_depth, options)?;
    let prepared = prepare_content(source, options, &cancelled)?;

    if border.x == 0 && border.y == 0 {
        return Ok(prepared);
    }

    if source_bit_depth == SourceBitDepth::Sixteen {
        return add_border_rgba16(
            &prepared,
            options,
            border,
            requirements,
            blur_mode,
            &cancelled,
        );
    }

    let source_rgba = prepared.to_rgba8();
    check_cancelled(&cancelled)?;
    let mut output = match options.border_style {
        BorderStyle::White => RgbaImage::from_pixel(
            requirements.output_width,
            requirements.output_height,
            Rgba([255; 4]),
        ),
        BorderStyle::Black => RgbaImage::from_pixel(
            requirements.output_width,
            requirements.output_height,
            Rgba([0, 0, 0, 255]),
        ),
        BorderStyle::MirroredBlur => blurred_edge_extension(
            &source_rgba,
            border,
            requirements.output_width,
            requirements.output_height,
            blur_mode,
            &cancelled,
        )?,
    };
    check_cancelled(&cancelled)?;

    paste(&mut output, &source_rgba, border.x, border.y, &cancelled)?;
    Ok(DynamicImage::ImageRgba8(output))
}

fn add_border_rgba16(
    source: &DynamicImage,
    options: &ProcessingOptions,
    border: BorderPixels,
    requirements: ProcessingRequirements,
    blur_mode: BlurMode,
    cancelled: &impl Fn() -> bool,
) -> Result<DynamicImage, ProcessingError> {
    let source_rgba = source.to_rgba16();
    check_cancelled(cancelled)?;
    let mut output = match options.border_style {
        BorderStyle::White => ImageBuffer::from_pixel(
            requirements.output_width,
            requirements.output_height,
            Rgba([65535; 4]),
        ),
        BorderStyle::Black => ImageBuffer::from_pixel(
            requirements.output_width,
            requirements.output_height,
            Rgba([0, 0, 0, 65535]),
        ),
        BorderStyle::MirroredBlur => blurred_edge_extension16(
            &source_rgba,
            border,
            requirements.output_width,
            requirements.output_height,
            blur_mode,
            cancelled,
        )?,
    };
    check_cancelled(cancelled)?;

    paste16(&mut output, &source_rgba, border.x, border.y, cancelled)?;
    Ok(DynamicImage::ImageRgba16(output))
}

fn check_cancelled(cancelled: &impl Fn() -> bool) -> Result<(), ProcessingError> {
    if cancelled() {
        Err(ProcessingError::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct ContentGeometry {
    target_width: u32,
    target_height: u32,
    ppi: Ppi,
}

pub fn crop_rect(
    width: u32,
    height: u32,
    print_size: PrintSizeMm,
) -> Result<CropRect, ProcessingError> {
    if width == 0 || height == 0 {
        return Err(ProcessingError::InvalidImageSize);
    }
    if !print_size.width.is_finite()
        || !print_size.height.is_finite()
        || print_size.width <= 0.0
        || print_size.height <= 0.0
    {
        return Err(ProcessingError::InvalidPrintSize);
    }

    let source_ratio = width as f64 / height as f64;
    let print_ratio = print_size.width as f64 / print_size.height as f64;
    let (crop_width, crop_height) = if source_ratio > print_ratio {
        (
            (height as f64 * print_ratio)
                .round()
                .clamp(1.0, width as f64) as u32,
            height,
        )
    } else {
        (
            width,
            (width as f64 / print_ratio)
                .round()
                .clamp(1.0, height as f64) as u32,
        )
    };
    Ok(CropRect {
        x: (width - crop_width) / 2,
        y: (height - crop_height) / 2,
        width: crop_width,
        height: crop_height,
    })
}

pub fn output_ppi(
    width: u32,
    height: u32,
    options: &ProcessingOptions,
) -> Result<Ppi, ProcessingError> {
    Ok(content_geometry(width, height, options)?.ppi)
}

fn content_geometry(
    width: u32,
    height: u32,
    options: &ProcessingOptions,
) -> Result<ContentGeometry, ProcessingError> {
    let crop = crop_rect(width, height, options.print_size)?;
    Ok(ContentGeometry {
        target_width: crop.width,
        target_height: crop.height,
        ppi: calculate_ppi(crop.width, crop.height, options.print_size)?,
    })
}

fn prepare_content(
    source: &DynamicImage,
    options: &ProcessingOptions,
    cancelled: &impl Fn() -> bool,
) -> Result<DynamicImage, ProcessingError> {
    check_cancelled(cancelled)?;
    let crop = crop_rect(source.width(), source.height(), options.print_size)?;
    let cropped = source.crop_imm(crop.x, crop.y, crop.width, crop.height);
    check_cancelled(cancelled)?;
    Ok(cropped)
}

fn premultiply_alpha8(
    image: &mut RgbaImage,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ProcessingError> {
    for y in 0..image.height() {
        check_cancelled(cancelled)?;
        for x in 0..image.width() {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
            let pixel = image.get_pixel_mut(x, y);
            let alpha = u16::from(pixel.0[3]);
            for channel in &mut pixel.0[..3] {
                *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
            }
        }
    }
    check_cancelled(cancelled)?;
    Ok(())
}

fn unpremultiply_alpha8(
    image: &mut RgbaImage,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ProcessingError> {
    for y in 0..image.height() {
        check_cancelled(cancelled)?;
        for x in 0..image.width() {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
            let pixel = image.get_pixel_mut(x, y);
            let alpha = u16::from(pixel.0[3]);
            for channel in &mut pixel.0[..3] {
                *channel = (u16::from(*channel) * 255 + alpha / 2)
                    .checked_div(alpha)
                    .unwrap_or(0)
                    .min(255) as u8;
            }
        }
    }
    check_cancelled(cancelled)?;
    Ok(())
}

fn premultiply_alpha16(
    image: &mut ImageBuffer<Rgba<u16>, Vec<u16>>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ProcessingError> {
    for y in 0..image.height() {
        check_cancelled(cancelled)?;
        for x in 0..image.width() {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
            let pixel = image.get_pixel_mut(x, y);
            let alpha = u64::from(pixel.0[3]);
            for channel in &mut pixel.0[..3] {
                *channel = ((u64::from(*channel) * alpha + 32767) / 65535) as u16;
            }
        }
    }
    check_cancelled(cancelled)?;
    Ok(())
}

fn unpremultiply_alpha16(
    image: &mut ImageBuffer<Rgba<u16>, Vec<u16>>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ProcessingError> {
    for y in 0..image.height() {
        check_cancelled(cancelled)?;
        for x in 0..image.width() {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
            let pixel = image.get_pixel_mut(x, y);
            let alpha = u64::from(pixel.0[3]);
            for channel in &mut pixel.0[..3] {
                *channel = (u64::from(*channel) * 65535 + alpha / 2)
                    .checked_div(alpha)
                    .unwrap_or(0)
                    .min(65535) as u16;
            }
        }
    }
    check_cancelled(cancelled)?;
    Ok(())
}

pub fn processing_requirements(
    width: u32,
    height: u32,
    source_bit_depth: SourceBitDepth,
    options: &ProcessingOptions,
) -> Result<ProcessingRequirements, ProcessingError> {
    processing_plan(width, height, source_bit_depth, options).map(|(_, requirements)| requirements)
}

fn processing_plan(
    width: u32,
    height: u32,
    source_bit_depth: SourceBitDepth,
    options: &ProcessingOptions,
) -> Result<(BorderPixels, ProcessingRequirements), ProcessingError> {
    let geometry = content_geometry(width, height, options)?;
    let border = border_pixels(options.border_mm, geometry.ppi)?;
    let output_width = geometry
        .target_width
        .checked_add(
            border
                .x
                .checked_mul(2)
                .ok_or(ProcessingError::OutputTooLarge)?,
        )
        .ok_or(ProcessingError::OutputTooLarge)?;
    let output_height = geometry
        .target_height
        .checked_add(
            border
                .y
                .checked_mul(2)
                .ok_or(ProcessingError::OutputTooLarge)?,
        )
        .ok_or(ProcessingError::OutputTooLarge)?;

    let (source_bytes_per_pixel, output_bytes_per_pixel) = match source_bit_depth {
        SourceBitDepth::Eight => (4_u64, 4_u64),
        SourceBitDepth::Sixteen => (8_u64, 8_u64),
        SourceBitDepth::Other => (16_u64, 4_u64),
    };
    let source_pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ProcessingError::OutputTooLarge)?;
    let source_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(source_bytes_per_pixel))
        .ok_or(ProcessingError::OutputTooLarge)?;
    let converted_source_bytes = source_pixels
        .checked_mul(source_bytes_per_pixel)
        .ok_or(ProcessingError::OutputTooLarge)?;
    let output_bytes = u64::from(output_width)
        .checked_mul(u64::from(output_height))
        .and_then(|pixels| pixels.checked_mul(output_bytes_per_pixel))
        .ok_or(ProcessingError::OutputTooLarge)?;
    let output_copies = match (border, options.border_style) {
        (BorderPixels { x: 0, y: 0 }, _) => 0,
        (_, BorderStyle::White | BorderStyle::Black) => 2,
        (_, BorderStyle::MirroredBlur) => 5,
    };
    let estimated_bytes = source_bytes
        .checked_add(converted_source_bytes)
        .and_then(|bytes| {
            output_bytes
                .checked_mul(output_copies)
                .and_then(|output| bytes.checked_add(output))
        })
        .ok_or(ProcessingError::OutputTooLarge)?
        .max(
            source_bytes
                .checked_mul(2)
                .ok_or(ProcessingError::OutputTooLarge)?,
        );
    if estimated_bytes > MAX_PROCESSING_BYTES {
        const MIB: u64 = 1024 * 1024;
        return Err(ProcessingError::ResourceLimit {
            estimated_mib: estimated_bytes.div_ceil(MIB),
            limit_mib: MAX_PROCESSING_BYTES / MIB,
        });
    }

    Ok((
        border,
        ProcessingRequirements {
            output_width,
            output_height,
            estimated_bytes,
        },
    ))
}

fn blurred_edge_extension(
    source: &RgbaImage,
    border: BorderPixels,
    output_width: u32,
    output_height: u32,
    blur_mode: BlurMode,
    cancelled: &impl Fn() -> bool,
) -> Result<RgbaImage, ProcessingError> {
    check_cancelled(cancelled)?;
    let source_width = source.width();
    let source_height = source.height();
    let mut extended = RgbaImage::new(output_width, output_height);

    for y in 0..output_height {
        check_cancelled(cancelled)?;
        for x in 0..output_width {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
            let source_x = reflected_source_coordinate(x, border.x, source_width);
            let source_y = reflected_source_coordinate(y, border.y, source_height);
            extended.put_pixel(x, y, *source.get_pixel(source_x, source_y));
        }
    }

    premultiply_alpha8(&mut extended, cancelled)?;
    let sigma = border.x.max(border.y).max(1) as f32 * 0.5;
    let blurred = match blur_mode {
        BlurMode::Exact => gaussian_blur_f32(&extended, sigma),
        BlurMode::Fast => image::imageops::fast_blur(&extended, sigma),
    };
    check_cancelled(cancelled)?;
    let mut feathered = extended.clone();
    check_cancelled(cancelled)?;

    for y in 0..output_height {
        check_cancelled(cancelled)?;
        for x in 0..output_width {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
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

    unpremultiply_alpha8(&mut feathered, cancelled)?;
    check_cancelled(cancelled)?;
    Ok(feathered)
}

fn reflected_source_coordinate(output: u32, border: u32, source_length: u32) -> u32 {
    let coordinate = i64::from(output) - i64::from(border);
    let source_length = i64::from(source_length);
    let wrapped = coordinate.rem_euclid(source_length * 2);
    if wrapped < source_length {
        wrapped as u32
    } else {
        (source_length * 2 - wrapped - 1) as u32
    }
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
    for (channel, output) in out.iter_mut().enumerate() {
        let edge = edge.0[channel] as f32;
        let blur = blur.0[channel] as f32;
        *output = (edge + (blur - edge) * weight).round().clamp(0.0, 255.0) as u8;
    }
    Rgba(out)
}

fn blurred_edge_extension16(
    source: &ImageBuffer<Rgba<u16>, Vec<u16>>,
    border: BorderPixels,
    output_width: u32,
    output_height: u32,
    blur_mode: BlurMode,
    cancelled: &impl Fn() -> bool,
) -> Result<ImageBuffer<Rgba<u16>, Vec<u16>>, ProcessingError> {
    check_cancelled(cancelled)?;
    let source_width = source.width();
    let source_height = source.height();
    let mut extended = ImageBuffer::new(output_width, output_height);

    for y in 0..output_height {
        check_cancelled(cancelled)?;
        for x in 0..output_width {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
            let source_x = reflected_source_coordinate(x, border.x, source_width);
            let source_y = reflected_source_coordinate(y, border.y, source_height);
            extended.put_pixel(x, y, *source.get_pixel(source_x, source_y));
        }
    }

    premultiply_alpha16(&mut extended, cancelled)?;
    let sigma = border.x.max(border.y).max(1) as f32 * 0.5;
    let blurred = match blur_mode {
        BlurMode::Exact => gaussian_blur_f32(&extended, sigma),
        BlurMode::Fast => image::imageops::fast_blur(&extended, sigma),
    };
    check_cancelled(cancelled)?;
    let mut feathered = extended.clone();
    check_cancelled(cancelled)?;

    for y in 0..output_height {
        check_cancelled(cancelled)?;
        for x in 0..output_width {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
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

    unpremultiply_alpha16(&mut feathered, cancelled)?;
    check_cancelled(cancelled)?;
    Ok(feathered)
}

fn blend_rgba16(edge: Rgba<u16>, blur: Rgba<u16>, weight: f32) -> Rgba<u16> {
    let mut out = [0; 4];
    for (channel, output) in out.iter_mut().enumerate() {
        let edge = edge.0[channel] as f32;
        let blur = blur.0[channel] as f32;
        *output = (edge + (blur - edge) * weight).round().clamp(0.0, 65535.0) as u16;
    }
    Rgba(out)
}

fn paste(
    output: &mut RgbaImage,
    source: &RgbaImage,
    offset_x: u32,
    offset_y: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ProcessingError> {
    for y in 0..source.height() {
        check_cancelled(cancelled)?;
        for x in 0..source.width() {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
            output.put_pixel(offset_x + x, offset_y + y, *source.get_pixel(x, y));
        }
    }
    check_cancelled(cancelled)?;
    Ok(())
}

fn paste16(
    output: &mut ImageBuffer<Rgba<u16>, Vec<u16>>,
    source: &ImageBuffer<Rgba<u16>, Vec<u16>>,
    offset_x: u32,
    offset_y: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ProcessingError> {
    for y in 0..source.height() {
        check_cancelled(cancelled)?;
        for x in 0..source.width() {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
            output.put_pixel(offset_x + x, offset_y + y, *source.get_pixel(x, y));
        }
    }
    check_cancelled(cancelled)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::reflected_source_coordinate;

    #[test]
    fn reflected_coordinates_repeat_source_away_from_both_edges() {
        let coordinates: Vec<_> = (0..10)
            .map(|output| reflected_source_coordinate(output, 3, 4))
            .collect();

        assert_eq!(coordinates, [2, 1, 0, 0, 1, 2, 3, 3, 2, 1]);
    }
}
