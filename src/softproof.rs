use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use image::{DynamicImage, ImageBuffer, Rgba, RgbaImage};
use lcms2::{ColorSpaceSignature, Flags, Intent, PixelFormat, Profile, Transform};
use thiserror::Error;

const MAX_ICC_PROFILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoftproofSettings {
    profile_path: Option<PathBuf>,
    enabled: bool,
}

impl Default for SoftproofSettings {
    fn default() -> Self {
        Self {
            profile_path: None,
            enabled: true,
        }
    }
}

impl SoftproofSettings {
    pub fn set_profile(&mut self, path: impl AsRef<Path>) {
        self.profile_path = Some(path.as_ref().to_path_buf());
    }

    pub fn clear_profile(&mut self) {
        self.profile_path = None;
    }

    pub fn profile_path(&self) -> Option<&Path> {
        self.profile_path.as_deref()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

#[derive(Debug, Error)]
pub enum SoftproofError {
    #[error("failed to load ICC profile `{path}`")]
    LoadProfile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to create ICC softproof transform")]
    CreateTransform(#[source] lcms2::Error),
    #[error("failed to load embedded source ICC profile")]
    LoadSourceProfile(#[source] lcms2::Error),
    #[error("embedded source ICC profile must use RGB color space")]
    SourceProfileNotRgb,
    #[error("output ICC profile `{path}` must use RGB color space")]
    OutputProfileNotRgb { path: PathBuf },
    #[error("ICC conversion cancelled")]
    Cancelled,
}

pub fn apply_preview_profile(
    image: &DynamicImage,
    settings: &SoftproofSettings,
) -> Result<DynamicImage, SoftproofError> {
    apply_preview_profile_with_source(image, settings, None)
}

pub fn apply_preview_profile_with_source(
    image: &DynamicImage,
    settings: &SoftproofSettings,
    source_icc_profile: Option<&[u8]>,
) -> Result<DynamicImage, SoftproofError> {
    if !settings.enabled() {
        return Ok(image.clone());
    }

    let Some(profile_path) = settings.profile_path() else {
        return Ok(image.clone());
    };

    let source_profile = source_profile_or_srgb(source_icc_profile)?;
    let display_profile = Profile::new_srgb();
    let proof_profile = load_proof_profile(profile_path)?;

    let transform = Transform::<u8, u8>::new_proofing(
        &source_profile,
        PixelFormat::RGB_8,
        &display_profile,
        PixelFormat::RGB_8,
        &proof_profile,
        Intent::Perceptual,
        Intent::Perceptual,
        Flags::SOFT_PROOFING | Flags::BLACKPOINT_COMPENSATION,
    )
    .map_err(SoftproofError::CreateTransform)?;

    Ok(apply_rgb_transform(image, &transform))
}

pub fn apply_source_profile_to_srgb(
    image: &DynamicImage,
    source_icc_profile: Option<&[u8]>,
) -> Result<DynamicImage, SoftproofError> {
    let Some(source_icc_profile) = source_icc_profile else {
        return Ok(image.clone());
    };
    let source_profile = source_profile_or_srgb(Some(source_icc_profile))?;
    let display_profile = Profile::new_srgb();
    let transform = Transform::<u8, u8>::new(
        &source_profile,
        PixelFormat::RGB_8,
        &display_profile,
        PixelFormat::RGB_8,
        Intent::Perceptual,
    )
    .map_err(SoftproofError::CreateTransform)?;

    Ok(apply_rgb_transform(image, &transform))
}

pub fn load_rgb_output_profile(path: &Path) -> Result<Vec<u8>, SoftproofError> {
    let profile_bytes = read_profile_bytes(path)?;
    let profile =
        Profile::new_icc(&profile_bytes).map_err(|error| SoftproofError::LoadProfile {
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidData, error),
        })?;
    if profile.color_space() != ColorSpaceSignature::RgbData {
        return Err(SoftproofError::OutputProfileNotRgb {
            path: path.to_path_buf(),
        });
    }
    Transform::<[u8; 3], [u8; 3]>::new(
        &Profile::new_srgb(),
        PixelFormat::RGB_8,
        &profile,
        PixelFormat::RGB_8,
        Intent::Perceptual,
    )
    .map_err(SoftproofError::CreateTransform)?;
    Ok(profile_bytes)
}

pub fn convert_to_output_profile(
    image: DynamicImage,
    source_icc_profile: Option<&[u8]>,
    output_icc_profile: &[u8],
    cancelled: impl Fn() -> bool,
) -> Result<DynamicImage, SoftproofError> {
    if cancelled() {
        return Err(SoftproofError::Cancelled);
    }
    let source_profile = source_profile_or_srgb(source_icc_profile)?;
    let output_profile =
        Profile::new_icc(output_icc_profile).map_err(|error| SoftproofError::LoadProfile {
            path: PathBuf::from("output profile"),
            source: io::Error::new(io::ErrorKind::InvalidData, error),
        })?;
    if output_profile.color_space() != ColorSpaceSignature::RgbData {
        return Err(SoftproofError::OutputProfileNotRgb {
            path: PathBuf::from("output profile"),
        });
    }

    match image {
        DynamicImage::ImageLuma16(_)
        | DynamicImage::ImageLumaA16(_)
        | DynamicImage::ImageRgb16(_)
        | DynamicImage::ImageRgba16(_) => {
            let transform = Transform::<[u16; 3], [u16; 3]>::new(
                &source_profile,
                PixelFormat::RGB_16,
                &output_profile,
                PixelFormat::RGB_16,
                Intent::Perceptual,
            )
            .map_err(SoftproofError::CreateTransform)?;
            convert_rgba16(image.into_rgba16(), &transform, &cancelled)
        }
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => {
            let transform = Transform::<[f32; 3], [f32; 3]>::new(
                &source_profile,
                PixelFormat::RGB_FLT,
                &output_profile,
                PixelFormat::RGB_FLT,
                Intent::Perceptual,
            )
            .map_err(SoftproofError::CreateTransform)?;
            convert_rgba32f(image.into_rgba32f(), &transform, &cancelled)
        }
        _ => {
            let transform = Transform::<[u8; 3], [u8; 3]>::new(
                &source_profile,
                PixelFormat::RGB_8,
                &output_profile,
                PixelFormat::RGB_8,
                Intent::Perceptual,
            )
            .map_err(SoftproofError::CreateTransform)?;
            convert_rgba8(image.into_rgba8(), &transform, &cancelled)
        }
    }
}

fn apply_rgb_transform(image: &DynamicImage, transform: &Transform<u8, u8>) -> DynamicImage {
    let rgba = image.to_rgba8();
    let mut source_rgb = Vec::with_capacity((rgba.width() * rgba.height() * 3) as usize);
    for pixel in rgba.pixels() {
        source_rgb.extend_from_slice(&pixel.0[..3]);
    }

    let mut proofed_rgb = vec![0_u8; source_rgb.len()];
    transform.transform_pixels(&source_rgb, &mut proofed_rgb);

    let mut output = RgbaImage::new(rgba.width(), rgba.height());
    for (index, pixel) in rgba.pixels().enumerate() {
        let rgb_index = index * 3;
        output.put_pixel(
            (index as u32) % rgba.width(),
            (index as u32) / rgba.width(),
            Rgba([
                proofed_rgb[rgb_index],
                proofed_rgb[rgb_index + 1],
                proofed_rgb[rgb_index + 2],
                pixel.0[3],
            ]),
        );
    }

    DynamicImage::ImageRgba8(output)
}

fn load_proof_profile(path: &Path) -> Result<Profile, SoftproofError> {
    let profile = read_profile_bytes(path)?;
    Profile::new_icc(&profile).map_err(|error| SoftproofError::LoadProfile {
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidData, error),
    })
}

fn source_profile_or_srgb(profile: Option<&[u8]>) -> Result<Profile, SoftproofError> {
    let Some(profile) = profile else {
        return Ok(Profile::new_srgb());
    };
    let profile = Profile::new_icc(profile).map_err(SoftproofError::LoadSourceProfile)?;
    if profile.color_space() != ColorSpaceSignature::RgbData {
        return Err(SoftproofError::SourceProfileNotRgb);
    }
    Ok(profile)
}

fn read_profile_bytes(path: &Path) -> Result<Vec<u8>, SoftproofError> {
    let load = || -> io::Result<Vec<u8>> {
        let file = File::open(path)?;
        let mut profile = Vec::new();
        file.take(MAX_ICC_PROFILE_BYTES + 1)
            .read_to_end(&mut profile)?;
        if profile.len() as u64 > MAX_ICC_PROFILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ICC profile exceeds 4 MiB limit",
            ));
        }
        Ok(profile)
    };
    load().map_err(|source| SoftproofError::LoadProfile {
        path: path.to_path_buf(),
        source,
    })
}

fn convert_rgba8(
    image: RgbaImage,
    transform: &Transform<[u8; 3], [u8; 3]>,
    cancelled: &impl Fn() -> bool,
) -> Result<DynamicImage, SoftproofError> {
    let mut source = Vec::with_capacity(image.width() as usize * image.height() as usize);
    for (index, pixel) in image.pixels().enumerate() {
        if index.is_multiple_of(1024) && cancelled() {
            return Err(SoftproofError::Cancelled);
        }
        source.push([pixel.0[0], pixel.0[1], pixel.0[2]]);
    }
    if cancelled() {
        return Err(SoftproofError::Cancelled);
    }
    let mut converted = vec![[0; 3]; source.len()];
    transform.transform_pixels(&source, &mut converted);
    if cancelled() {
        return Err(SoftproofError::Cancelled);
    }
    let mut output = RgbaImage::new(image.width(), image.height());
    for (index, (source, converted)) in image.pixels().zip(converted).enumerate() {
        if index.is_multiple_of(1024) && cancelled() {
            return Err(SoftproofError::Cancelled);
        }
        output.put_pixel(
            index as u32 % image.width(),
            index as u32 / image.width(),
            Rgba([converted[0], converted[1], converted[2], source.0[3]]),
        );
    }
    Ok(DynamicImage::ImageRgba8(output))
}

fn convert_rgba16(
    image: ImageBuffer<Rgba<u16>, Vec<u16>>,
    transform: &Transform<[u16; 3], [u16; 3]>,
    cancelled: &impl Fn() -> bool,
) -> Result<DynamicImage, SoftproofError> {
    let mut source = Vec::with_capacity(image.width() as usize * image.height() as usize);
    for (index, pixel) in image.pixels().enumerate() {
        if index.is_multiple_of(1024) && cancelled() {
            return Err(SoftproofError::Cancelled);
        }
        source.push([pixel.0[0], pixel.0[1], pixel.0[2]]);
    }
    if cancelled() {
        return Err(SoftproofError::Cancelled);
    }
    let mut converted = vec![[0; 3]; source.len()];
    transform.transform_pixels(&source, &mut converted);
    if cancelled() {
        return Err(SoftproofError::Cancelled);
    }
    let mut output = ImageBuffer::new(image.width(), image.height());
    for (index, (source, converted)) in image.pixels().zip(converted).enumerate() {
        if index.is_multiple_of(1024) && cancelled() {
            return Err(SoftproofError::Cancelled);
        }
        output.put_pixel(
            index as u32 % image.width(),
            index as u32 / image.width(),
            Rgba([converted[0], converted[1], converted[2], source.0[3]]),
        );
    }
    Ok(DynamicImage::ImageRgba16(output))
}

fn convert_rgba32f(
    image: image::Rgba32FImage,
    transform: &Transform<[f32; 3], [f32; 3]>,
    cancelled: &impl Fn() -> bool,
) -> Result<DynamicImage, SoftproofError> {
    let mut source = Vec::with_capacity(image.width() as usize * image.height() as usize);
    for (index, pixel) in image.pixels().enumerate() {
        if index.is_multiple_of(1024) && cancelled() {
            return Err(SoftproofError::Cancelled);
        }
        source.push([pixel.0[0], pixel.0[1], pixel.0[2]]);
    }
    if cancelled() {
        return Err(SoftproofError::Cancelled);
    }
    let mut converted = vec![[0.0; 3]; source.len()];
    transform.transform_pixels(&source, &mut converted);
    if cancelled() {
        return Err(SoftproofError::Cancelled);
    }
    let mut output = image::Rgba32FImage::new(image.width(), image.height());
    for (index, (source, converted)) in image.pixels().zip(converted).enumerate() {
        if index.is_multiple_of(1024) && cancelled() {
            return Err(SoftproofError::Cancelled);
        }
        output.put_pixel(
            index as u32 % image.width(),
            index as u32 / image.width(),
            Rgba([converted[0], converted[1], converted[2], source.0[3]]),
        );
    }
    Ok(DynamicImage::ImageRgba32F(output))
}
