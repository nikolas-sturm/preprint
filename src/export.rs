use std::{
    borrow::Cow,
    io::{self, BufWriter, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use image::{
    DynamicImage, ExtendedColorType, ImageEncoder, ImageError, ImageFormat,
    codecs::jpeg::{JpegEncoder, PixelDensity as JpegPixelDensity, PixelDensityUnit},
};
use lcms2::{ColorSpaceSignature, Profile};
use rust_i18n::t;
use thiserror::Error;
use tiff::encoder::colortype;
use tiff::{
    encoder::{Rational, TiffValue},
    tags::{ExtraSamples, ResolutionUnit, Tag, Type},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::loader::SourceBitDepth;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("failed to create output file")]
    CreateFile(#[source] std::io::Error),
    #[error("failed to write output file")]
    WriteFile(#[source] std::io::Error),
    #[error("failed to sync output file")]
    SyncFile(#[source] std::io::Error),
    #[error("output file already exists: `{0}`")]
    OutputExists(PathBuf),
    #[error("failed to move completed output file into place")]
    PersistFile(#[source] std::io::Error),
    #[error("export cancelled")]
    Cancelled,
    #[error("failed to encode image")]
    Encode(#[source] ImageError),
    #[error("failed to encode PNG")]
    PngEncode(#[source] png::EncodingError),
    #[error("failed to encode TIFF")]
    TiffEncode(#[source] tiff::TiffError),
    #[error("16-bit TIFF requires 16-bit input")]
    SixteenBitTiffRequiresSixteenBitInput,
    #[error("16-bit output is only supported for TIFF")]
    SixteenBitOnlySupportedForTiff,
    #[error("failed to create sRGB ICC profile: {0}")]
    CreateColorProfile(String),
    #[error("embedded source ICC profile is invalid or not RGB")]
    InvalidColorProfile,
    #[error("encoder does not support ICC profile embedding")]
    EmbedColorProfile(#[source] image::error::UnsupportedError),
}

struct CancellationWriter<'a, W, F> {
    inner: &'a mut W,
    cancelled: &'a F,
    cancel_observed: &'a AtomicBool,
}

impl<W: Write, F: Fn() -> bool> Write for CancellationWriter<'_, W, F> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if (self.cancelled)() {
            self.cancel_observed.store(true, Ordering::Release);
            return Err(io::Error::other("export cancelled"));
        }
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        if (self.cancelled)() {
            self.cancel_observed.store(true, Ordering::Release);
            return Err(io::Error::other("export cancelled"));
        }
        self.inner.flush()
    }
}

impl<W: Seek, F: Fn() -> bool> Seek for CancellationWriter<'_, W, F> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if (self.cancelled)() {
            self.cancel_observed.store(true, Ordering::Release);
            return Err(io::Error::other("export cancelled"));
        }
        self.inner.seek(position)
    }
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
        crate::i18n::translate(self.translation_key())
    }

    pub fn label_for_locale(self, locale: &str) -> String {
        crate::i18n::translate_for_locale(self.translation_key(), locale)
    }

    const fn translation_key(self) -> &'static str {
        match self {
            Self::Fast => "deflate-fast",
            Self::Balanced => "deflate-balanced",
            Self::Best => "deflate-best",
        }
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
    pub pixel_density: Option<PixelDensity>,
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
            pixel_density: None,
        }
    }

    pub const fn with_pixel_density(mut self, x: u32, y: u32) -> Self {
        self.pixel_density = Some(PixelDensity {
            x: if x == 0 { 1 } else { x },
            y: if y == 0 { 1 } else { y },
        });
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelDensity {
    pub x: u32,
    pub y: u32,
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
        crate::i18n::translate(key)
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
    let locale = crate::i18n::current_language();
    compression_preview_label_for_locale(options, &locale)
}

pub fn compression_preview_label_for_locale(options: &ExportOptions, locale: &str) -> String {
    match options.format {
        OutputFormat::Jpeg => t!(
            "compression-jpeg",
            locale = locale,
            quality = options.quality.clamp(1, 100)
        )
        .into_owned(),
        OutputFormat::Png => t!(
            "compression-png",
            locale = locale,
            level = options.png_compression.clamp(1, 9)
        )
        .into_owned(),
        OutputFormat::Tiff => match options.tiff_compression {
            TiffCompression::Lzw => t!("compression-tiff-lzw", locale = locale).into_owned(),
            TiffCompression::Deflate => t!(
                "compression-tiff-zip",
                locale = locale,
                level = options.tiff_deflate_level.label_for_locale(locale)
            )
            .into_owned(),
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
            encode_jpeg(
                &image,
                &mut bytes,
                options.quality,
                options.pixel_density,
                None,
            )?;
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
    save_image_with_icc_profile_and_cancel(image, output_path, options, None, || false)
}

pub fn save_image_with_cancel(
    image: &DynamicImage,
    output_path: &Path,
    options: &ExportOptions,
    cancelled: impl Fn() -> bool,
) -> Result<(), ExportError> {
    save_image_with_icc_profile_and_cancel(image, output_path, options, None, cancelled)
}

pub fn save_image_with_icc_profile_and_cancel(
    image: &DynamicImage,
    output_path: &Path,
    options: &ExportOptions,
    source_icc_profile: Option<&[u8]>,
    cancelled: impl Fn() -> bool,
) -> Result<(), ExportError> {
    if options.bit_depth == BitDepth::Sixteen && options.format != OutputFormat::Tiff {
        return Err(ExportError::SixteenBitOnlySupportedForTiff);
    }
    if cancelled() {
        return Err(ExportError::Cancelled);
    }
    let icc_profile = output_icc_profile(source_icc_profile)?;

    let output_dir = output_path.parent().unwrap_or_else(|| Path::new("."));
    let mut builder = tempfile::Builder::new();
    builder.prefix(".preprint-");
    #[cfg(unix)]
    builder.permissions(std::fs::Permissions::from_mode(0o666));
    let mut temporary = builder
        .tempfile_in(output_dir)
        .map_err(ExportError::CreateFile)?;

    let cancel_observed = AtomicBool::new(false);
    let encode_result = {
        let mut buffered = BufWriter::new(temporary.as_file_mut());
        let result = {
            let mut writer = CancellationWriter {
                inner: &mut buffered,
                cancelled: &cancelled,
                cancel_observed: &cancel_observed,
            };
            match options.format {
                OutputFormat::Png => save_png(
                    image,
                    &mut writer,
                    options.png_compression,
                    options.pixel_density,
                    icc_profile,
                ),
                OutputFormat::Tiff => save_tiff(
                    image,
                    &mut writer,
                    options.bit_depth,
                    options.tiff_compression,
                    options.tiff_deflate_level,
                    options.pixel_density,
                    icc_profile,
                ),
                OutputFormat::Jpeg => encode_jpeg(
                    image,
                    &mut writer,
                    options.quality.clamp(1, 100),
                    options.pixel_density,
                    Some(icc_profile),
                ),
            }
        };
        result.and_then(|()| buffered.flush().map_err(ExportError::WriteFile))
    };
    if let Err(error) = encode_result {
        if cancel_observed.load(Ordering::Acquire) {
            return Err(ExportError::Cancelled);
        }
        return Err(error);
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(ExportError::SyncFile)?;
    if cancelled() {
        return Err(ExportError::Cancelled);
    }

    match temporary.persist_noclobber(output_path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(ExportError::OutputExists(output_path.to_path_buf()))
        }
        Err(error) => Err(ExportError::PersistFile(error.error)),
    }
}

fn save_png<W: Write>(
    image: &DynamicImage,
    writer: W,
    level: u8,
    pixel_density: Option<PixelDensity>,
    icc_profile: &[u8],
) -> Result<(), ExportError> {
    let rgba8 = image.to_rgba8();
    let mut info = png::Info::with_size(rgba8.width(), rgba8.height());
    info.color_type = png::ColorType::Rgba;
    info.bit_depth = png::BitDepth::Eight;
    info.icc_profile = Some(Cow::Borrowed(icc_profile));
    let mut encoder = png::Encoder::with_info(writer, info).map_err(ExportError::PngEncode)?;
    encoder.set_deflate_compression(png::DeflateCompression::Level(level.clamp(1, 9)));
    encoder.set_filter(png::Filter::Adaptive);
    if let Some(density) = pixel_density {
        encoder.set_pixel_dims(Some(png::PixelDimensions {
            xppu: pixels_per_meter(density.x),
            yppu: pixels_per_meter(density.y),
            unit: png::Unit::Meter,
        }));
    }
    let mut writer = encoder.write_header().map_err(ExportError::PngEncode)?;
    writer
        .write_image_data(rgba8.as_raw())
        .map_err(ExportError::PngEncode)
}

fn save_tiff<W: Write + Seek>(
    image: &DynamicImage,
    writer: W,
    bit_depth: BitDepth,
    compression: TiffCompression,
    deflate_level: TiffDeflateLevel,
    pixel_density: Option<PixelDensity>,
    icc_profile: &[u8],
) -> Result<(), ExportError> {
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
            let mut image = encoder
                .new_image::<colortype::RGBA8>(rgba8.width(), rgba8.height())
                .map_err(ExportError::TiffEncode)?;
            set_tiff_metadata(&mut image, pixel_density, icc_profile)
                .map_err(ExportError::TiffEncode)?;
            image
                .write_data(rgba8.as_raw())
                .map_err(ExportError::TiffEncode)
        }
        BitDepth::Sixteen => {
            if !is_16_bit_image(image) {
                return Err(ExportError::SixteenBitTiffRequiresSixteenBitInput);
            }
            let rgba16 = image.to_rgba16();
            let mut image = encoder
                .new_image::<colortype::RGBA16>(rgba16.width(), rgba16.height())
                .map_err(ExportError::TiffEncode)?;
            set_tiff_metadata(&mut image, pixel_density, icc_profile)
                .map_err(ExportError::TiffEncode)?;
            image
                .write_data(rgba16.as_raw())
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

fn encode_jpeg<W: Write>(
    image: &DynamicImage,
    writer: W,
    quality: u8,
    pixel_density: Option<PixelDensity>,
    icc_profile: Option<&[u8]>,
) -> Result<(), ExportError> {
    let rgb = image.to_rgb8();
    let mut encoder = JpegEncoder::new_with_quality(writer, quality.clamp(1, 100));
    if let Some(density) = pixel_density {
        encoder.set_pixel_density(JpegPixelDensity {
            density: (
                density.x.min(u32::from(u16::MAX)) as u16,
                density.y.min(u32::from(u16::MAX)) as u16,
            ),
            unit: PixelDensityUnit::Inches,
        });
    }
    if let Some(icc_profile) = icc_profile {
        encoder
            .set_icc_profile(icc_profile.to_vec())
            .map_err(ExportError::EmbedColorProfile)?;
    }

    encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(ExportError::Encode)
}

fn pixels_per_meter(ppi: u32) -> u32 {
    u32::try_from((u64::from(ppi) * 10_000 + 127) / 254).unwrap_or(u32::MAX)
}

fn set_tiff_metadata<W, C, K>(
    image: &mut tiff::encoder::ImageEncoder<'_, W, C, K>,
    pixel_density: Option<PixelDensity>,
    icc_profile: &[u8],
) -> Result<(), tiff::TiffError>
where
    W: Write + Seek,
    C: tiff::encoder::colortype::ColorType,
    K: tiff::encoder::TiffKind,
{
    if let Some(density) = pixel_density {
        image
            .encoder()
            .write_tag(Tag::ResolutionUnit, ResolutionUnit::Inch)?;
        image
            .encoder()
            .write_tag(Tag::XResolution, Rational { n: density.x, d: 1 })?;
        image
            .encoder()
            .write_tag(Tag::YResolution, Rational { n: density.y, d: 1 })?;
    }
    image
        .encoder()
        .write_tag(Tag::ExtraSamples, &[ExtraSamples::UnassociatedAlpha][..])?;
    image
        .encoder()
        .write_tag(Tag::IccProfile, IccProfile(icc_profile))?;
    Ok(())
}

fn output_icc_profile(source_icc_profile: Option<&[u8]>) -> Result<&[u8], ExportError> {
    if let Some(profile) = source_icc_profile
        && Profile::new_icc(profile)
            .is_ok_and(|profile| profile.color_space() == ColorSpaceSignature::RgbData)
    {
        return Ok(profile);
    }
    if source_icc_profile.is_some() {
        return Err(ExportError::InvalidColorProfile);
    }

    static SRGB_ICC_PROFILE: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    SRGB_ICC_PROFILE
        .get_or_init(|| Profile::new_srgb().icc().map_err(|error| error.to_string()))
        .as_deref()
        .map_err(|error| ExportError::CreateColorProfile(error.clone()))
}

struct IccProfile<'a>(&'a [u8]);

impl TiffValue for IccProfile<'_> {
    const BYTE_LEN: u8 = 1;
    const FIELD_TYPE: Type = Type::UNDEFINED;

    fn count(&self) -> usize {
        self.0.len()
    }

    fn data(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(self.0)
    }
}
