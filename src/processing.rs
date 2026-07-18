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
    #[error("target PPI must be between 1 and 9600")]
    InvalidTargetPpi,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResizeMode {
    #[default]
    NoResize,
    Fit,
    Fill,
}

impl ResizeMode {
    pub const ALL: [Self; 3] = [Self::NoResize, Self::Fit, Self::Fill];

    pub fn label(self) -> String {
        crate::i18n::translate(match self {
            Self::NoResize => "resize-none",
            Self::Fit => "resize-fit",
            Self::Fill => "resize-fill",
        })
    }
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
    pub resize_mode: ResizeMode,
    pub target_ppi: u32,
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
            resize_mode: ResizeMode::NoResize,
            target_ppi: 300,
        }
    }

    pub const fn with_resizing(mut self, resize_mode: ResizeMode, target_ppi: u32) -> Self {
        self.resize_mode = resize_mode;
        self.target_ppi = target_ppi;
        self
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
    check_cancelled(&cancelled)?;
    let (width, height) = source.dimensions();
    let source_bit_depth = SourceBitDepth::from_image(source);
    let (border, requirements) = processing_plan(width, height, source_bit_depth, options)?;
    let prepared = prepare_content(source, options, &cancelled)?;

    if border.x == 0 && border.y == 0 {
        return Ok(prepared);
    }

    if source_bit_depth == SourceBitDepth::Sixteen {
        return add_border_rgba16(&prepared, options, border, requirements, &cancelled);
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
    resized_width: u32,
    resized_height: u32,
    ppi: Ppi,
}

pub fn target_pixel_dimensions(
    print_size: PrintSizeMm,
    target_ppi: u32,
) -> Result<(u32, u32), ProcessingError> {
    if !print_size.width.is_finite()
        || !print_size.height.is_finite()
        || print_size.width <= 0.0
        || print_size.height <= 0.0
    {
        return Err(ProcessingError::InvalidPrintSize);
    }
    if !(1..=9600).contains(&target_ppi) {
        return Err(ProcessingError::InvalidTargetPpi);
    }
    let pixels = |millimeters: f32| -> Result<u32, ProcessingError> {
        let value = (millimeters / MM_PER_INCH) * target_ppi as f32;
        if !value.is_finite() || value <= 0.0 || value > u32::MAX as f32 {
            return Err(ProcessingError::OutputTooLarge);
        }
        Ok(value.round().max(1.0) as u32)
    };
    Ok((pixels(print_size.width)?, pixels(print_size.height)?))
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
    if width == 0 || height == 0 {
        return Err(ProcessingError::InvalidImageSize);
    }
    if options.resize_mode == ResizeMode::NoResize {
        return Ok(ContentGeometry {
            target_width: width,
            target_height: height,
            resized_width: width,
            resized_height: height,
            ppi: calculate_ppi(width, height, options.print_size)?,
        });
    }

    let (target_width, target_height) =
        target_pixel_dimensions(options.print_size, options.target_ppi)?;
    let width_scale = target_width as f64 / width as f64;
    let height_scale = target_height as f64 / height as f64;
    let scale = match options.resize_mode {
        ResizeMode::Fit => width_scale.min(height_scale),
        ResizeMode::Fill => width_scale.max(height_scale),
        ResizeMode::NoResize => unreachable!(),
    };
    let scaled = |value: u32| -> Result<u32, ProcessingError> {
        let value = (value as f64 * scale).round().max(1.0);
        if !value.is_finite() || value > u32::MAX as f64 {
            return Err(ProcessingError::OutputTooLarge);
        }
        Ok(value as u32)
    };
    Ok(ContentGeometry {
        target_width,
        target_height,
        resized_width: scaled(width)?,
        resized_height: scaled(height)?,
        ppi: calculate_ppi(target_width, target_height, options.print_size)?,
    })
}

fn prepare_content(
    source: &DynamicImage,
    options: &ProcessingOptions,
    cancelled: &impl Fn() -> bool,
) -> Result<DynamicImage, ProcessingError> {
    check_cancelled(cancelled)?;
    if options.resize_mode == ResizeMode::NoResize {
        let source = source.clone();
        check_cancelled(cancelled)?;
        return Ok(source);
    }
    let geometry = content_geometry(source.width(), source.height(), options)?;
    let resized = resize_with_premultiplied_alpha(
        source,
        geometry.resized_width,
        geometry.resized_height,
        cancelled,
    )?;
    match options.resize_mode {
        ResizeMode::Fit => fit_to_canvas(&resized, geometry, options.border_style, cancelled),
        ResizeMode::Fill => {
            let x = geometry.resized_width.saturating_sub(geometry.target_width) / 2;
            let y = geometry
                .resized_height
                .saturating_sub(geometry.target_height)
                / 2;
            let cropped = resized.crop_imm(x, y, geometry.target_width, geometry.target_height);
            check_cancelled(cancelled)?;
            Ok(cropped)
        }
        ResizeMode::NoResize => unreachable!(),
    }
}

fn resize_with_premultiplied_alpha(
    source: &DynamicImage,
    width: u32,
    height: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<DynamicImage, ProcessingError> {
    check_cancelled(cancelled)?;
    match source {
        DynamicImage::ImageLumaA8(_) | DynamicImage::ImageRgba8(_) => {
            let mut image = source.to_rgba32f();
            check_cancelled(cancelled)?;
            premultiply_alpha32f(&mut image, cancelled)?;
            let mut resized = DynamicImage::ImageRgba32F(image)
                .resize_exact(width, height, image::imageops::FilterType::Lanczos3)
                .to_rgba32f();
            check_cancelled(cancelled)?;
            unpremultiply_alpha32f(&mut resized, cancelled)?;
            let resized = DynamicImage::ImageRgba32F(resized).to_rgba8();
            check_cancelled(cancelled)?;
            Ok(DynamicImage::ImageRgba8(resized))
        }
        DynamicImage::ImageLumaA16(_) | DynamicImage::ImageRgba16(_) => {
            let mut image = source.to_rgba32f();
            check_cancelled(cancelled)?;
            premultiply_alpha32f(&mut image, cancelled)?;
            let mut resized = DynamicImage::ImageRgba32F(image)
                .resize_exact(width, height, image::imageops::FilterType::Lanczos3)
                .to_rgba32f();
            check_cancelled(cancelled)?;
            unpremultiply_alpha32f(&mut resized, cancelled)?;
            let resized = DynamicImage::ImageRgba32F(resized).to_rgba16();
            check_cancelled(cancelled)?;
            Ok(DynamicImage::ImageRgba16(resized))
        }
        DynamicImage::ImageRgba32F(image) => {
            let mut image = image.clone();
            check_cancelled(cancelled)?;
            premultiply_alpha32f(&mut image, cancelled)?;
            let mut resized = DynamicImage::ImageRgba32F(image)
                .resize_exact(width, height, image::imageops::FilterType::Lanczos3)
                .to_rgba32f();
            check_cancelled(cancelled)?;
            unpremultiply_alpha32f(&mut resized, cancelled)?;
            Ok(DynamicImage::ImageRgba32F(resized))
        }
        _ => {
            let resized = source.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
            check_cancelled(cancelled)?;
            Ok(resized)
        }
    }
}

fn premultiply_alpha32f(
    image: &mut ImageBuffer<Rgba<f32>, Vec<f32>>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ProcessingError> {
    for y in 0..image.height() {
        check_cancelled(cancelled)?;
        for x in 0..image.width() {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
            let pixel = image.get_pixel_mut(x, y);
            let alpha = pixel.0[3];
            for channel in &mut pixel.0[..3] {
                *channel *= alpha;
            }
        }
    }
    check_cancelled(cancelled)?;
    Ok(())
}

fn unpremultiply_alpha32f(
    image: &mut ImageBuffer<Rgba<f32>, Vec<f32>>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ProcessingError> {
    for y in 0..image.height() {
        check_cancelled(cancelled)?;
        for x in 0..image.width() {
            if x.is_multiple_of(1024) {
                check_cancelled(cancelled)?;
            }
            let pixel = image.get_pixel_mut(x, y);
            let alpha = pixel.0[3];
            if alpha <= f32::EPSILON {
                pixel.0[..3].fill(0.0);
            } else {
                for channel in &mut pixel.0[..3] {
                    *channel = (*channel / alpha).clamp(0.0, 1.0);
                }
            }
        }
    }
    check_cancelled(cancelled)?;
    Ok(())
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

fn fit_to_canvas(
    source: &DynamicImage,
    geometry: ContentGeometry,
    style: BorderStyle,
    cancelled: &impl Fn() -> bool,
) -> Result<DynamicImage, ProcessingError> {
    check_cancelled(cancelled)?;
    let offset = BorderPixels {
        x: geometry.target_width.saturating_sub(geometry.resized_width) / 2,
        y: geometry
            .target_height
            .saturating_sub(geometry.resized_height)
            / 2,
    };
    if SourceBitDepth::from_image(source) == SourceBitDepth::Sixteen {
        let source = source.to_rgba16();
        check_cancelled(cancelled)?;
        let mut output = match style {
            BorderStyle::White => ImageBuffer::from_pixel(
                geometry.target_width,
                geometry.target_height,
                Rgba([65535; 4]),
            ),
            BorderStyle::Black => ImageBuffer::from_pixel(
                geometry.target_width,
                geometry.target_height,
                Rgba([0, 0, 0, 65535]),
            ),
            BorderStyle::MirroredBlur => blurred_edge_extension16(
                &source,
                offset,
                geometry.target_width,
                geometry.target_height,
                cancelled,
            )?,
        };
        check_cancelled(cancelled)?;
        paste16(&mut output, &source, offset.x, offset.y, cancelled)?;
        return Ok(DynamicImage::ImageRgba16(output));
    }

    let source = source.to_rgba8();
    check_cancelled(cancelled)?;
    let mut output = match style {
        BorderStyle::White => RgbaImage::from_pixel(
            geometry.target_width,
            geometry.target_height,
            Rgba([255; 4]),
        ),
        BorderStyle::Black => RgbaImage::from_pixel(
            geometry.target_width,
            geometry.target_height,
            Rgba([0, 0, 0, 255]),
        ),
        BorderStyle::MirroredBlur => blurred_edge_extension(
            &source,
            offset,
            geometry.target_width,
            geometry.target_height,
            cancelled,
        )?,
    };
    check_cancelled(cancelled)?;
    paste(&mut output, &source, offset.x, offset.y, cancelled)?;
    Ok(DynamicImage::ImageRgba8(output))
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
    let resize_bytes_per_pixel = match options.resize_mode {
        ResizeMode::NoResize => output_bytes_per_pixel,
        ResizeMode::Fit | ResizeMode::Fill => 16_u64,
    };
    let converted_source_bytes_per_pixel = match options.resize_mode {
        ResizeMode::NoResize => source_bytes_per_pixel,
        ResizeMode::Fit | ResizeMode::Fill => resize_bytes_per_pixel,
    };
    let source_pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ProcessingError::OutputTooLarge)?;
    let source_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(source_bytes_per_pixel))
        .ok_or(ProcessingError::OutputTooLarge)?;
    let converted_source_bytes = source_pixels
        .checked_mul(converted_source_bytes_per_pixel)
        .ok_or(ProcessingError::OutputTooLarge)?;
    let output_bytes = u64::from(output_width)
        .checked_mul(u64::from(output_height))
        .and_then(|pixels| pixels.checked_mul(output_bytes_per_pixel))
        .ok_or(ProcessingError::OutputTooLarge)?;
    let resize_bytes = if options.resize_mode == ResizeMode::NoResize {
        0
    } else {
        let resized = u64::from(geometry.resized_width)
            .checked_mul(u64::from(geometry.resized_height))
            .and_then(|pixels| pixels.checked_mul(resize_bytes_per_pixel))
            .ok_or(ProcessingError::OutputTooLarge)?;
        let content = u64::from(geometry.target_width)
            .checked_mul(u64::from(geometry.target_height))
            .and_then(|pixels| pixels.checked_mul(resize_bytes_per_pixel))
            .ok_or(ProcessingError::OutputTooLarge)?;
        let vertical_filter = u64::from(width)
            .checked_mul(u64::from(geometry.resized_height))
            .and_then(|pixels| pixels.checked_mul(16))
            .ok_or(ProcessingError::OutputTooLarge)?;
        let horizontal_filter = u64::from(geometry.resized_width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(16))
            .ok_or(ProcessingError::OutputTooLarge)?;
        let filter_intermediate = vertical_filter.max(horizontal_filter);
        resized
            .checked_mul(2)
            .and_then(|bytes| {
                content
                    .checked_mul(2)
                    .and_then(|content| bytes.checked_add(content))
            })
            .and_then(|bytes| bytes.checked_add(filter_intermediate))
            .ok_or(ProcessingError::OutputTooLarge)?
    };
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
        .and_then(|bytes| bytes.checked_add(resize_bytes))
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
    let blurred = gaussian_blur_f32(&extended, border.x.max(border.y).max(1) as f32 * 0.5);
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
    let blurred = gaussian_blur_f32(&extended, border.x.max(border.y).max(1) as f32 * 0.5);
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
