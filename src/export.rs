use std::{
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use image::{
    DynamicImage, ExtendedColorType, ImageEncoder, ImageError, ImageFormat,
    codecs::jpeg::JpegEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use thiserror::Error;
use tiff::encoder::colortype;

use crate::loader::SourceBitDepth;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("failed to create output file")]
    CreateFile(#[source] std::io::Error),
    #[error("failed to encode image")]
    Encode(#[source] ImageError),
    #[error("failed to encode TIFF")]
    TiffEncode(#[source] tiff::TiffError),
    #[error("16-bit TIFF requires 16-bit input")]
    SixteenBitTiffRequiresSixteenBitInput,
    #[error("16-bit output is only supported for TIFF")]
    SixteenBitOnlySupportedForTiff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Png,
    Jpeg,
    Tiff,
}

impl OutputFormat {
    pub const ALL: [Self; 3] = [Self::Png, Self::Jpeg, Self::Tiff];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Tiff => "TIFF",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TiffCompression {
    Lzw,
    Deflate,
}

impl TiffCompression {
    pub const ALL: [Self; 2] = [Self::Lzw, Self::Deflate];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Lzw => "LZW",
            Self::Deflate => "ZIP (Deflate)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TiffDeflateLevel {
    Fast,
    Balanced,
    Best,
}

impl TiffDeflateLevel {
    pub const ALL: [Self; 3] = [Self::Fast, Self::Balanced, Self::Best];

    pub fn label(self) -> String {
        let key = match self {
            Self::Fast => "deflate-fast",
            Self::Balanced => "deflate-balanced",
            Self::Best => "deflate-best",
        };
        egui_i18n::tr!(key)
    }

    const fn to_tiff(self) -> tiff::encoder::DeflateLevel {
        match self {
            Self::Fast => tiff::encoder::DeflateLevel::Fast,
            Self::Balanced => tiff::encoder::DeflateLevel::Balanced,
            Self::Best => tiff::encoder::DeflateLevel::Best,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    pub format: OutputFormat,
    pub quality: u8,
    pub bit_depth: BitDepth,
    pub png_compression: u8,
    pub tiff_compression: TiffCompression,
    pub tiff_deflate_level: TiffDeflateLevel,
}

impl ExportOptions {
    pub const fn new(format: OutputFormat, quality: u8) -> Self {
        Self {
            format,
            quality,
            bit_depth: BitDepth::Eight,
            png_compression: 6,
            tiff_compression: TiffCompression::Deflate,
            tiff_deflate_level: TiffDeflateLevel::Balanced,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitDepth {
    Eight,
    Sixteen,
}

impl BitDepth {
    pub fn label(self) -> String {
        let key = match self {
            Self::Eight => "bit-depth-8",
            Self::Sixteen => "bit-depth-16",
        };
        egui_i18n::tr!(key)
    }
}

pub const fn extension(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Png => "png",
        OutputFormat::Jpeg => "jpg",
        OutputFormat::Tiff => "tiff",
    }
}

pub fn can_export_bit_depth(source: SourceBitDepth, options: &ExportOptions) -> bool {
    match options.bit_depth {
        BitDepth::Eight => true,
        BitDepth::Sixteen => {
            options.format == OutputFormat::Tiff && source == SourceBitDepth::Sixteen
        }
    }
}

pub fn compression_preview_label(options: &ExportOptions) -> String {
    match options.format {
        OutputFormat::Jpeg => egui_i18n::tr!(
            "compression-jpeg",
            { quality: options.quality.clamp(1, 100) as i32 }
        ),
        OutputFormat::Png => egui_i18n::tr!(
            "compression-png",
            { level: options.png_compression.clamp(1, 9) as i32 }
        ),
        OutputFormat::Tiff => match options.tiff_compression {
            TiffCompression::Lzw => egui_i18n::tr!("compression-tiff-lzw"),
            TiffCompression::Deflate => egui_i18n::tr!(
                "compression-tiff-zip",
                { level: options.tiff_deflate_level.label() }
            ),
        },
    }
}

pub fn compression_preview_image(
    image: DynamicImage,
    options: &ExportOptions,
) -> Result<DynamicImage, ExportError> {
    match options.format {
        OutputFormat::Jpeg => {
            let mut bytes = Vec::new();
            encode_jpeg(&image, &mut bytes, options.quality)?;
            image::load_from_memory_with_format(&bytes, ImageFormat::Jpeg)
                .map_err(ExportError::Encode)
        }
        OutputFormat::Png | OutputFormat::Tiff => Ok(image),
    }
}

pub fn unique_output_path(input_path: &Path, output_dir: &Path, format: OutputFormat) -> PathBuf {
    let stem = input_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("image");
    let extension = extension(format);

    for index in 0.. {
        let suffix = if index == 0 {
            String::new()
        } else {
            format!("_{index}")
        };
        let candidate = output_dir.join(format!("{stem}_preprint{suffix}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!("unbounded output path search should always return")
}

pub fn save_image(
    image: &DynamicImage,
    output_path: &Path,
    options: &ExportOptions,
) -> Result<(), ExportError> {
    if options.bit_depth == BitDepth::Sixteen && options.format != OutputFormat::Tiff {
        return Err(ExportError::SixteenBitOnlySupportedForTiff);
    }

    match options.format {
        OutputFormat::Png => save_png(image, output_path, options.png_compression),
        OutputFormat::Tiff => save_tiff(
            image,
            output_path,
            options.bit_depth,
            options.tiff_compression,
            options.tiff_deflate_level,
        ),
        OutputFormat::Jpeg => save_jpeg(image, output_path, options.quality.clamp(1, 100)),
    }
}

fn save_png(image: &DynamicImage, output_path: &Path, level: u8) -> Result<(), ExportError> {
    let file = File::create(output_path).map_err(ExportError::CreateFile)?;
    let writer = BufWriter::new(file);
    let encoder = PngEncoder::new_with_quality(
        writer,
        CompressionType::Level(level.clamp(1, 9)),
        FilterType::Adaptive,
    );

    if is_16_bit_image(image) {
        let rgba16 = image.to_rgba16();
        let raw: &[u16] = rgba16.as_raw();
        let bytes: &[u8] = bytemuck::cast_slice(raw);
        encoder
            .write_image(
                bytes,
                rgba16.width(),
                rgba16.height(),
                ExtendedColorType::Rgba16,
            )
            .map_err(ExportError::Encode)
    } else {
        let rgba8 = image.to_rgba8();
        encoder
            .write_image(
                rgba8.as_raw(),
                rgba8.width(),
                rgba8.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(ExportError::Encode)
    }
}

fn save_tiff(
    image: &DynamicImage,
    output_path: &Path,
    bit_depth: BitDepth,
    compression: TiffCompression,
    deflate_level: TiffDeflateLevel,
) -> Result<(), ExportError> {
    let file = File::create(output_path).map_err(ExportError::CreateFile)?;
    let writer = BufWriter::new(file);

    let tiff_compression = match compression {
        TiffCompression::Lzw => tiff::encoder::Compression::Lzw,
        TiffCompression::Deflate => tiff::encoder::Compression::Deflate(deflate_level.to_tiff()),
    };

    let mut encoder = tiff::encoder::TiffEncoder::new(writer)
        .map_err(ExportError::TiffEncode)?
        .with_compression(tiff_compression);

    match bit_depth {
        BitDepth::Eight => {
            let rgba8 = image.to_rgba8();
            encoder
                .write_image::<colortype::RGBA8>(rgba8.width(), rgba8.height(), rgba8.as_raw())
                .map_err(ExportError::TiffEncode)
        }
        BitDepth::Sixteen => {
            if !is_16_bit_image(image) {
                return Err(ExportError::SixteenBitTiffRequiresSixteenBitInput);
            }
            let rgba16 = image.to_rgba16();
            encoder
                .write_image::<colortype::RGBA16>(rgba16.width(), rgba16.height(), rgba16.as_raw())
                .map_err(ExportError::TiffEncode)
        }
    }
}

fn is_16_bit_image(image: &DynamicImage) -> bool {
    matches!(
        image,
        DynamicImage::ImageLuma16(_)
            | DynamicImage::ImageLumaA16(_)
            | DynamicImage::ImageRgb16(_)
            | DynamicImage::ImageRgba16(_)
    )
}

fn save_jpeg(image: &DynamicImage, output_path: &Path, quality: u8) -> Result<(), ExportError> {
    let file = File::create(output_path).map_err(ExportError::CreateFile)?;
    let writer = BufWriter::new(file);
    encode_jpeg(image, writer, quality)
}

fn encode_jpeg<W: Write>(image: &DynamicImage, writer: W, quality: u8) -> Result<(), ExportError> {
    let rgb = image.to_rgb8();
    let mut encoder = JpegEncoder::new_with_quality(writer, quality.clamp(1, 100));

    encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(ExportError::Encode)
}
