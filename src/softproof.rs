use std::path::{Path, PathBuf};

use image::{DynamicImage, Rgba, RgbaImage};
use lcms2::{Flags, Intent, PixelFormat, Profile, Transform};
use thiserror::Error;

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
}

pub fn apply_preview_profile(
    image: &DynamicImage,
    settings: &SoftproofSettings,
) -> Result<DynamicImage, SoftproofError> {
    if !settings.enabled() {
        return Ok(image.clone());
    }

    let Some(profile_path) = settings.profile_path() else {
        return Ok(image.clone());
    };

    let source_profile = Profile::new_srgb();
    let display_profile = Profile::new_srgb();
    let proof_profile =
        Profile::new_file(profile_path).map_err(|source| SoftproofError::LoadProfile {
            path: profile_path.to_path_buf(),
            source,
        })?;

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

    Ok(DynamicImage::ImageRgba8(output))
}
