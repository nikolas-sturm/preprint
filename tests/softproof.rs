use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use preprint::softproof::{SoftproofSettings, apply_preview_profile};
use tempfile::tempdir;

#[test]
fn stores_selected_icc_profile_path() {
    let mut settings = SoftproofSettings::default();
    settings.set_profile("/tmp/printshop.icc");

    assert_eq!(
        settings.profile_path().unwrap().to_string_lossy(),
        "/tmp/printshop.icc"
    );
}

#[test]
fn softproof_is_enabled_by_default_and_can_be_toggled() {
    let mut settings = SoftproofSettings::default();

    assert!(settings.enabled());
    settings.set_enabled(false);
    assert!(!settings.enabled());
}

#[test]
fn preview_profile_transform_preserves_image_without_selected_profile() {
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255])));
    let settings = SoftproofSettings::default();

    let preview = apply_preview_profile(&image, &settings).unwrap();

    assert_eq!(preview.to_rgba8().get_pixel(0, 0), &Rgba([1, 2, 3, 255]));
}

#[test]
fn reports_error_when_selected_icc_profile_cannot_be_loaded() {
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255])));
    let mut settings = SoftproofSettings::default();
    settings.set_profile("/tmp/preprint-missing-profile.icc");

    let err = apply_preview_profile(&image, &settings).unwrap_err();

    assert!(err.to_string().contains("failed to load ICC profile"));
}

#[test]
fn disabled_softproof_skips_selected_icc_profile() {
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255])));
    let mut settings = SoftproofSettings::default();
    settings.set_profile("/tmp/preprint-missing-profile.icc");
    settings.set_enabled(false);

    let preview = apply_preview_profile(&image, &settings).unwrap();

    assert_eq!(preview.to_rgba8().get_pixel(0, 0), &Rgba([1, 2, 3, 255]));
}

#[test]
fn accepts_valid_srgb_icc_profile_for_softproof_preview() {
    let dir = tempdir().unwrap();
    let profile_path = dir.path().join("srgb.icc");
    let profile = lcms2::Profile::new_srgb();
    std::fs::write(&profile_path, profile.icc().unwrap()).unwrap();

    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255])));
    let mut settings = SoftproofSettings::default();
    settings.set_profile(&profile_path);

    let preview = apply_preview_profile(&image, &settings).unwrap();

    assert_eq!(preview.dimensions(), (1, 1));
}
