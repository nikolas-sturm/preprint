use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Mutex, TryLockError},
    thread,
    time::Duration,
};

use image::{
    ColorType, DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits,
    metadata::Orientation,
};
use lcms2::{ColorSpaceSignature, Profile};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct LoadedImage {
    pub image: DynamicImage,
    pub bit_depth: SourceBitDepth,
    pub format: Option<ImageFormat>,
    pub icc_profile: Option<Vec<u8>>,
}

const MAX_ICC_PROFILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DECODER_HEADER_BYTES: u64 = MAX_ICC_PROFILE_BYTES as u64 * 2 + 1024 * 1024;
const MAX_BUFFERED_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DECODER_OVERHEAD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DECODED_IMAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
static DECODER_SETUP_LOCK: Mutex<()> = Mutex::new(());

struct BoundedReader<R> {
    inner: R,
    position: u64,
    limit: u64,
}

impl<R> BoundedReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            position: 0,
            limit,
        }
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.position);
        let length = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buffer[..length])?;
        self.position += read as u64;
        Ok(read)
    }
}

impl<R: BufRead> BufRead for BoundedReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let remaining = self.limit.saturating_sub(self.position);
        let buffer = self.inner.fill_buf()?;
        let length = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        Ok(&buffer[..length])
    }

    fn consume(&mut self, amount: usize) {
        let amount = amount
            .min(usize::try_from(self.limit.saturating_sub(self.position)).unwrap_or(usize::MAX));
        self.inner.consume(amount);
        self.position += amount as u64;
    }
}

impl<R: Seek> Seek for BoundedReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let position = match position {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.limit) + i128::from(offset),
        };
        if !(0..=i128::from(self.limit)).contains(&position) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek exceeds snapshotted image length",
            ));
        }
        let position = position as u64;
        self.inner.seek(SeekFrom::Start(position))?;
        self.position = position;
        Ok(position)
    }
}

#[derive(Clone, Debug)]
pub struct ImageMetadata {
    pub dimensions: (u32, u32),
    pub bit_depth: SourceBitDepth,
    pub format: Option<ImageFormat>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceBitDepth {
    Eight,
    Sixteen,
    Other,
}

impl SourceBitDepth {
    pub fn from_image(image: &DynamicImage) -> Self {
        match image {
            DynamicImage::ImageLuma8(_)
            | DynamicImage::ImageLumaA8(_)
            | DynamicImage::ImageRgb8(_)
            | DynamicImage::ImageRgba8(_) => Self::Eight,
            DynamicImage::ImageLuma16(_)
            | DynamicImage::ImageLumaA16(_)
            | DynamicImage::ImageRgb16(_)
            | DynamicImage::ImageRgba16(_) => Self::Sixteen,
            _ => Self::Other,
        }
    }

    pub const fn from_color_type(color_type: ColorType) -> Self {
        match color_type {
            ColorType::L8 | ColorType::La8 | ColorType::Rgb8 | ColorType::Rgba8 => Self::Eight,
            ColorType::L16 | ColorType::La16 | ColorType::Rgb16 | ColorType::Rgba16 => {
                Self::Sixteen
            }
            _ => Self::Other,
        }
    }

    pub fn label(self) -> String {
        let key = match self {
            Self::Eight => "bit-depth-8",
            Self::Sixteen => "bit-depth-16",
            Self::Other => "source-other-depth",
        };
        crate::i18n::translate(key)
    }
}

#[derive(Debug, Error)]
pub enum LoadImageError {
    #[error("failed to open `{path}`")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to detect image format for `{path}`")]
    GuessFormat {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to decode `{path}`{format}")]
    Decode {
        path: PathBuf,
        format: FormatHint,
        #[source]
        source: ImageError,
    },
    #[error("failed to read metadata for `{path}`{format}")]
    ReadMetadata {
        path: PathBuf,
        format: FormatHint,
        #[source]
        source: ImageError,
    },
    #[error("image `{path}` cannot be processed: {reason}")]
    Rejected { path: PathBuf, reason: String },
}

#[derive(Debug)]
pub struct FormatHint(Option<ImageFormat>);

impl std::fmt::Display for FormatHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(format) => write!(f, " as {format:?}"),
            None => Ok(()),
        }
    }
}

pub fn load_image(path: &Path) -> Result<LoadedImage, LoadImageError> {
    load_image_with_reservations(path, || false, |_| Ok(()), |_| Ok(())).map(|(image, ())| image)
}

pub fn load_image_with_reservation<T>(
    path: &Path,
    reserve: impl FnOnce(&ImageMetadata) -> Result<T, String>,
) -> Result<(LoadedImage, T), LoadImageError> {
    let metadata = load_image_metadata(path)?;
    let reservation = reserve(&metadata).map_err(|reason| LoadImageError::Rejected {
        path: path.to_path_buf(),
        reason,
    })?;
    let image = load_image(path)?;
    if (image.image.width(), image.image.height()) != metadata.dimensions
        || image.bit_depth != metadata.bit_depth
    {
        return Err(LoadImageError::Rejected {
            path: path.to_path_buf(),
            reason: "image changed while it was being loaded".to_owned(),
        });
    }
    Ok((image, reservation))
}

pub(crate) fn load_image_with_reservations<I, T>(
    path: &Path,
    cancelled: impl Fn() -> bool,
    reserve_input: impl FnOnce(u64) -> Result<I, String>,
    reserve: impl FnOnce(&ImageMetadata) -> Result<T, String>,
) -> Result<(LoadedImage, T), LoadImageError> {
    let decoder_lock = loop {
        match DECODER_SETUP_LOCK.try_lock() {
            Ok(lock) => break lock,
            Err(TryLockError::Poisoned(error)) => break error.into_inner(),
            Err(TryLockError::WouldBlock) if cancelled() => {
                return Err(LoadImageError::Rejected {
                    path: path.to_path_buf(),
                    reason: "image loading cancelled".to_owned(),
                });
            }
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(10)),
        }
    };
    let file = File::open(path).map_err(|source| LoadImageError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let file_size = file
        .metadata()
        .map_err(|source| LoadImageError::Open {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let reader = ImageReader::new(BoundedReader::new(BufReader::new(file), file_size));

    let mut reader =
        reader
            .with_guessed_format()
            .map_err(|source| LoadImageError::GuessFormat {
                path: path.to_path_buf(),
                source,
            })?;
    let mut header_limits = Limits::default();
    header_limits.max_alloc = Some(MAX_DECODER_HEADER_BYTES);
    reader.limits(header_limits);
    let format = reader.format();
    let input_reservation_bytes = decoder_input_reservation_bytes(path, format, file_size)?;
    let input_reservation =
        reserve_input(input_reservation_bytes).map_err(|reason| LoadImageError::Rejected {
            path: path.to_path_buf(),
            reason,
        })?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|source| LoadImageError::Decode {
            path: path.to_path_buf(),
            format: FormatHint(format),
            source,
        })?;
    let orientation = decoder
        .orientation()
        .map_err(|source| LoadImageError::ReadMetadata {
            path: path.to_path_buf(),
            format: FormatHint(format),
            source,
        })?;
    let metadata = ImageMetadata {
        dimensions: oriented_dimensions(decoder.dimensions(), orientation),
        bit_depth: SourceBitDepth::from_color_type(decoder.color_type()),
        format,
    };
    let reservation = reserve(&metadata).map_err(|reason| LoadImageError::Rejected {
        path: path.to_path_buf(),
        reason,
    })?;

    let decoded_bytes = decoder.total_bytes();
    let original_bytes = u64::from(decoder.dimensions().0)
        .checked_mul(u64::from(decoder.dimensions().1))
        .and_then(|pixels| {
            pixels
                .checked_mul(u64::from(decoder.original_color_type().bits_per_pixel()).div_ceil(8))
        })
        .unwrap_or(u64::MAX);
    let decoder_bytes = if format == Some(ImageFormat::Tiff) {
        original_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(decoded_bytes))
    } else {
        Some(decoded_bytes)
    };
    let peak_bytes = if orientation_requires_copy(orientation) {
        decoder_bytes.and_then(|bytes| bytes.checked_add(decoded_bytes))
    } else {
        decoder_bytes
    };
    if peak_bytes
        .and_then(|bytes| bytes.checked_add(input_reservation_bytes))
        .is_none_or(|combined| combined > MAX_DECODED_IMAGE_BYTES)
    {
        return Err(LoadImageError::Rejected {
            path: path.to_path_buf(),
            reason: format!("decoder and image require more than {MAX_DECODED_IMAGE_BYTES} bytes"),
        });
    }
    let mut metadata_limits = Limits::no_limits();
    metadata_limits.max_alloc = decoded_bytes
        .max(original_bytes)
        .checked_add(MAX_ICC_PROFILE_BYTES as u64);
    decoder
        .set_limits(metadata_limits)
        .map_err(|source| LoadImageError::Decode {
            path: path.to_path_buf(),
            format: FormatHint(format),
            source,
        })?;
    let icc_profile = decoder
        .icc_profile()
        .map_err(|source| LoadImageError::Decode {
            path: path.to_path_buf(),
            format: FormatHint(format),
            source,
        })?;
    let icc_profile = usable_rgb_icc_profile(path, decoder.color_type(), icc_profile)?;
    let mut decode_limits = Limits::no_limits();
    decode_limits.max_alloc = decoded_bytes
        .max(original_bytes)
        .checked_add(MAX_DECODER_OVERHEAD_BYTES);
    decoder
        .set_limits(decode_limits)
        .map_err(|source| LoadImageError::Decode {
            path: path.to_path_buf(),
            format: FormatHint(format),
            source,
        })?;
    let mut image =
        DynamicImage::from_decoder(decoder).map_err(|source| LoadImageError::Decode {
            path: path.to_path_buf(),
            format: FormatHint(format),
            source,
        })?;
    image.apply_orientation(orientation);
    let bit_depth = SourceBitDepth::from_image(&image);
    drop(input_reservation);
    drop(decoder_lock);

    Ok((
        LoadedImage {
            image,
            bit_depth,
            format,
            icc_profile,
        },
        reservation,
    ))
}

fn decoder_input_reservation_bytes(
    path: &Path,
    format: Option<ImageFormat>,
    file_size: u64,
) -> Result<u64, LoadImageError> {
    if matches!(format, Some(ImageFormat::Jpeg | ImageFormat::WebP)) {
        if file_size > MAX_BUFFERED_INPUT_BYTES {
            return Err(LoadImageError::Rejected {
                path: path.to_path_buf(),
                reason: format!(
                    "encoded image is {file_size} bytes; buffered decoder limit is {MAX_BUFFERED_INPUT_BYTES} bytes"
                ),
            });
        }
        Ok(file_size
            .saturating_mul(2)
            .saturating_add(MAX_DECODER_OVERHEAD_BYTES))
    } else {
        Ok(file_size
            .min(MAX_DECODER_HEADER_BYTES)
            .saturating_add(MAX_DECODER_OVERHEAD_BYTES))
    }
}

fn usable_rgb_icc_profile(
    path: &Path,
    color_type: ColorType,
    icc_profile: Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>, LoadImageError> {
    let Some(icc_profile) = icc_profile else {
        return Ok(None);
    };
    if icc_profile.len() > MAX_ICC_PROFILE_BYTES {
        return Err(LoadImageError::Rejected {
            path: path.to_path_buf(),
            reason: format!(
                "embedded ICC profile is {} bytes; limit is {MAX_ICC_PROFILE_BYTES} bytes",
                icc_profile.len()
            ),
        });
    }
    let rgb_pixels = matches!(
        color_type,
        ColorType::Rgb8
            | ColorType::Rgba8
            | ColorType::Rgb16
            | ColorType::Rgba16
            | ColorType::Rgb32F
            | ColorType::Rgba32F
    );
    let profile = Profile::new_icc(&icc_profile).map_err(|error| LoadImageError::Rejected {
        path: path.to_path_buf(),
        reason: format!("embedded ICC profile is invalid: {error}"),
    })?;
    if !rgb_pixels || profile.color_space() != ColorSpaceSignature::RgbData {
        return Err(LoadImageError::Rejected {
            path: path.to_path_buf(),
            reason: "embedded ICC profile is incompatible with decoded pixel format".to_owned(),
        });
    }
    Ok(Some(icc_profile))
}

pub fn load_image_metadata(path: &Path) -> Result<ImageMetadata, LoadImageError> {
    let _decoder_lock = DECODER_SETUP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let file = File::open(path).map_err(|source| LoadImageError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let file_size = file
        .metadata()
        .map_err(|source| LoadImageError::Open {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let reader = ImageReader::new(BoundedReader::new(BufReader::new(file), file_size));

    let mut reader =
        reader
            .with_guessed_format()
            .map_err(|source| LoadImageError::GuessFormat {
                path: path.to_path_buf(),
                source,
            })?;
    let mut header_limits = Limits::default();
    header_limits.max_alloc = Some(MAX_DECODER_HEADER_BYTES);
    reader.limits(header_limits);
    let format = reader.format();
    decoder_input_reservation_bytes(path, format, file_size)?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|source| LoadImageError::ReadMetadata {
            path: path.to_path_buf(),
            format: FormatHint(format),
            source,
        })?;
    let orientation = decoder
        .orientation()
        .map_err(|source| LoadImageError::ReadMetadata {
            path: path.to_path_buf(),
            format: FormatHint(format),
            source,
        })?;

    Ok(ImageMetadata {
        dimensions: oriented_dimensions(decoder.dimensions(), orientation),
        bit_depth: SourceBitDepth::from_color_type(decoder.color_type()),
        format,
    })
}

fn oriented_dimensions(dimensions: (u32, u32), orientation: Orientation) -> (u32, u32) {
    match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (dimensions.1, dimensions.0),
        _ => dimensions,
    }
}

fn orientation_requires_copy(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}
