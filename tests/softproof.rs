use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use preprint::softproof::{
    SoftproofError, SoftproofSettings, apply_preview_profile, apply_preview_profile_with_source,
    apply_source_profile_to_srgb, convert_to_output_profile, load_rgb_output_profile,
};
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

#[test]
fn invalid_embedded_source_profile_is_rejected() {
    let dir = tempdir().unwrap();
    let profile_path = dir.path().join("srgb.icc");
    std::fs::write(&profile_path, lcms2::Profile::new_srgb().icc().unwrap()).unwrap();
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255])));
    let mut settings = SoftproofSettings::default();
    settings.set_profile(&profile_path);

    let error = apply_preview_profile_with_source(&image, &settings, Some(b"invalid")).unwrap_err();

    assert!(error.to_string().contains("embedded source ICC profile"));
}

#[test]
fn converts_profiled_preview_to_srgb_display_space() {
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([31, 127, 223, 200])));
    let profile = lcms2::Profile::new_srgb().icc().unwrap();

    let display = apply_source_profile_to_srgb(&image, Some(&profile)).unwrap();

    assert_eq!(
        display.to_rgba8().get_pixel(0, 0),
        &Rgba([31, 127, 223, 200])
    );
}

#[test]
fn rejects_oversized_softproof_profile() {
    let dir = tempdir().unwrap();
    let profile_path = dir.path().join("oversized.icc");
    std::fs::write(&profile_path, vec![0; 4 * 1024 * 1024 + 1]).unwrap();
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255])));
    let mut settings = SoftproofSettings::default();
    settings.set_profile(&profile_path);

    let error = apply_preview_profile(&image, &settings).unwrap_err();

    assert!(error.to_string().contains("failed to load ICC profile"));
}

#[test]
fn output_profile_conversion_preserves_eight_bit_alpha() {
    let profile = lcms2::Profile::new_srgb().icc().unwrap();
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([31, 127, 223, 77])));

    let converted = convert_to_output_profile(image, Some(&profile), &profile, || false).unwrap();

    assert_eq!(
        converted.to_rgba8().get_pixel(0, 0),
        &Rgba([31, 127, 223, 77])
    );
}

#[test]
fn output_profile_conversion_preserves_sixteen_bit_precision_and_alpha() {
    let profile = lcms2::Profile::new_srgb().icc().unwrap();
    let image = DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
        1,
        1,
        Rgba([8_000, 32_000, 56_000, 12_345]),
    ));

    let converted = convert_to_output_profile(image, Some(&profile), &profile, || false).unwrap();

    assert!(matches!(converted, DynamicImage::ImageRgba16(_)));
    assert_eq!(
        converted.to_rgba16().get_pixel(0, 0),
        &Rgba([8_000, 32_000, 56_000, 12_345])
    );
}

#[test]
fn output_profile_conversion_honors_cancellation() {
    let profile = lcms2::Profile::new_srgb().icc().unwrap();
    let image = DynamicImage::new_rgba8(2, 2);

    let error = convert_to_output_profile(image, None, &profile, || true).unwrap_err();

    assert!(matches!(error, SoftproofError::Cancelled));
}

#[test]
fn output_profile_loader_rejects_non_rgb_profile() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("xyz.icc");
    std::fs::write(&path, lcms2::Profile::new_xyz().icc().unwrap()).unwrap();

    let error = load_rgb_output_profile(&path).unwrap_err();

    assert!(error.to_string().contains("must use RGB color space"));
}
