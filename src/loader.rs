use std::path::{Path, PathBuf};

use image::{ColorType, DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct LoadedImage {
    pub image: DynamicImage,
    pub bit_depth: SourceBitDepth,
    pub format: Option<ImageFormat>,
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
        egui_i18n::tr!(key)
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
    let reader = ImageReader::open(path).map_err(|source| LoadImageError::Open {
        path: path.to_path_buf(),
        source,
    })?;

    let reader = reader
        .with_guessed_format()
        .map_err(|source| LoadImageError::GuessFormat {
            path: path.to_path_buf(),
            source,
        })?;
    let format = reader.format();
    let image = reader.decode().map_err(|source| LoadImageError::Decode {
        path: path.to_path_buf(),
        format: FormatHint(format),
        source,
    })?;
    let bit_depth = SourceBitDepth::from_image(&image);

    Ok(LoadedImage {
        image,
        bit_depth,
        format,
    })
}

pub fn load_image_metadata(path: &Path) -> Result<ImageMetadata, LoadImageError> {
    let reader = ImageReader::open(path).map_err(|source| LoadImageError::Open {
        path: path.to_path_buf(),
        source,
    })?;

    let reader = reader
        .with_guessed_format()
        .map_err(|source| LoadImageError::GuessFormat {
            path: path.to_path_buf(),
            source,
        })?;
    let format = reader.format();
    let decoder = reader
        .into_decoder()
        .map_err(|source| LoadImageError::ReadMetadata {
            path: path.to_path_buf(),
            format: FormatHint(format),
            source,
        })?;

    Ok(ImageMetadata {
        dimensions: decoder.dimensions(),
        bit_depth: SourceBitDepth::from_color_type(decoder.color_type()),
        format,
    })
}
