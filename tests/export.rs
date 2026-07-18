use image::{DynamicImage, ImageDecoder, Rgba, RgbaImage};
use preprint::{
    export::{
        BitDepth, ExportOptions, OutputFormat, can_export_bit_depth, compression_preview_image,
        compression_preview_label_for_locale, extension, save_image, save_image_with_cancel,
        save_image_with_icc_profile_and_cancel, unique_output_path,
    },
    loader::SourceBitDepth,
};
use tempfile::tempdir;

fn test_image() -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([20, 40, 60, 255])))
}

fn embedded_icc_profile(path: &std::path::Path, format: OutputFormat) -> Vec<u8> {
    match format {
        OutputFormat::Png => {
            let decoder =
                png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
            decoder
                .read_info()
                .unwrap()
                .info()
                .icc_profile
                .as_ref()
                .unwrap()
                .to_vec()
        }
        OutputFormat::Jpeg => {
            let mut decoder = image::codecs::jpeg::JpegDecoder::new(std::io::BufReader::new(
                std::fs::File::open(path).unwrap(),
            ))
            .unwrap();
            decoder.icc_profile().unwrap().unwrap()
        }
        OutputFormat::Tiff => {
            let mut decoder =
                tiff::decoder::Decoder::new(std::fs::File::open(path).unwrap()).unwrap();
            decoder
                .get_tag(tiff::tags::Tag::IccProfile)
                .unwrap()
                .into_u8_vec()
                .unwrap()
        }
    }
}

#[test]
fn maps_output_formats_to_extensions() {
    assert_eq!(extension(OutputFormat::Png), "png");
    assert_eq!(extension(OutputFormat::Jpeg), "jpg");
    assert_eq!(extension(OutputFormat::Tiff), "tiff");
}

#[test]
fn creates_non_overwriting_output_paths() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("scan one.png");
    let first = dir.path().join("scan one_preprint.png");
    let second = dir.path().join("scan one_preprint_1.png");
    std::fs::write(&first, b"existing").unwrap();

    let output = unique_output_path(&input, dir.path(), OutputFormat::Png);

    assert_eq!(output, second);
}

#[test]
fn saves_png_jpeg_and_tiff_files() {
    let dir = tempdir().unwrap();
    let image = test_image();
    let cases = [
        (OutputFormat::Png, "out.png"),
        (OutputFormat::Jpeg, "out.jpg"),
        (OutputFormat::Tiff, "out.tiff"),
    ];

    for (format, name) in cases {
        let path = dir.path().join(name);
        save_image(&image, &path, &ExportOptions::new(format, 90)).unwrap();
        assert!(path.exists(), "{} should be written", path.display());
        assert!(std::fs::metadata(path).unwrap().len() > 0);
    }
}

#[test]
fn exports_embed_valid_srgb_profile_by_default() {
    let dir = tempdir().unwrap();
    for (format, name) in [
        (OutputFormat::Png, "out.png"),
        (OutputFormat::Jpeg, "out.jpg"),
        (OutputFormat::Tiff, "out.tiff"),
    ] {
        let path = dir.path().join(name);
        save_image(&test_image(), &path, &ExportOptions::new(format, 90)).unwrap();

        let profile = embedded_icc_profile(&path, format);
        let profile = lcms2::Profile::new_icc(&profile).unwrap();
        assert_eq!(profile.color_space(), lcms2::ColorSpaceSignature::RgbData);
    }
}

#[test]
fn exports_preserve_valid_source_rgb_profile() {
    let dir = tempdir().unwrap();
    let profile = lcms2::Profile::new_srgb();
    profile.set_encoded_icc_version(0x0210_0000);
    let profile = profile.icc().unwrap();

    for (format, name) in [
        (OutputFormat::Png, "source.png"),
        (OutputFormat::Jpeg, "source.jpg"),
        (OutputFormat::Tiff, "source.tiff"),
    ] {
        let path = dir.path().join(name);
        save_image_with_icc_profile_and_cancel(
            &test_image(),
            &path,
            &ExportOptions::new(format, 90),
            Some(&profile),
            || false,
        )
        .unwrap();

        assert_eq!(embedded_icc_profile(&path, format), profile);
    }
}

#[test]
fn invalid_source_profile_is_rejected() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("fallback.png");

    let error = save_image_with_icc_profile_and_cancel(
        &test_image(),
        &path,
        &ExportOptions::new(OutputFormat::Png, 90),
        Some(b"invalid"),
        || false,
    )
    .unwrap_err();

    assert!(error.to_string().contains("invalid or not RGB"));
    assert!(!path.exists());
}

#[test]
fn png_records_pixel_density() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("out.png");
    let options = ExportOptions::new(OutputFormat::Png, 90).with_pixel_density(300, 240);

    save_image(&test_image(), &path, &options).unwrap();

    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let reader = decoder.read_info().unwrap();
    let dimensions = reader.info().pixel_dims.unwrap();
    assert_eq!(dimensions.unit, png::Unit::Meter);
    assert_eq!(dimensions.xppu, 11_811);
    assert_eq!(dimensions.yppu, 9_449);
}

#[test]
fn jpeg_records_pixel_density() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("out.jpg");
    let options = ExportOptions::new(OutputFormat::Jpeg, 90).with_pixel_density(300, 240);

    save_image(&test_image(), &path, &options).unwrap();

    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], &[0xff, 0xd8, 0xff, 0xe0]);
    assert_eq!(&bytes[6..11], b"JFIF\0");
    assert_eq!(bytes[13], 1);
    assert_eq!(u16::from_be_bytes([bytes[14], bytes[15]]), 300);
    assert_eq!(u16::from_be_bytes([bytes[16], bytes[17]]), 240);
}

#[test]
fn tiff_records_pixel_density() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("out.tiff");
    let options = ExportOptions::new(OutputFormat::Tiff, 90).with_pixel_density(300, 240);

    save_image(&test_image(), &path, &options).unwrap();

    let mut decoder = tiff::decoder::Decoder::new(std::fs::File::open(path).unwrap()).unwrap();
    assert_eq!(
        decoder
            .get_tag_u32(tiff::tags::Tag::ResolutionUnit)
            .unwrap(),
        2
    );
    assert!(matches!(
        decoder.get_tag(tiff::tags::Tag::XResolution).unwrap(),
        tiff::decoder::ifd::Value::Rational(300, 1)
    ));
    assert!(matches!(
        decoder.get_tag(tiff::tags::Tag::YResolution).unwrap(),
        tiff::decoder::ifd::Value::Rational(240, 1)
    ));
    assert_eq!(
        decoder
            .get_tag_u16_vec(tiff::tags::Tag::ExtraSamples)
            .unwrap(),
        [2]
    );
}

#[test]
fn rejects_16_bit_tiff_for_8_bit_sources() {
    let mut options = ExportOptions::new(OutputFormat::Tiff, 90);
    options.bit_depth = BitDepth::Sixteen;

    assert!(!can_export_bit_depth(SourceBitDepth::Eight, &options));

    let dir = tempdir().unwrap();
    let err = save_image(&test_image(), &dir.path().join("out.tiff"), &options).unwrap_err();
    assert!(
        err.to_string()
            .contains("16-bit TIFF requires 16-bit input")
    );
}

#[test]
fn saves_real_16_bit_tiff_for_16_bit_image() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("out.tiff");
    let image = DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
        2,
        1,
        image::Rgba([1000, 2000, 3000, 65535]),
    ));
    let mut options = ExportOptions::new(OutputFormat::Tiff, 90);
    options.bit_depth = BitDepth::Sixteen;

    assert!(can_export_bit_depth(SourceBitDepth::Sixteen, &options));
    save_image(&image, &path, &options).unwrap();

    let decoded = image::open(&path).unwrap();
    assert!(matches!(
        decoded,
        DynamicImage::ImageRgba16(_) | DynamicImage::ImageRgb16(_)
    ));
}

#[test]
fn saves_16_bit_source_as_8_bit_png_when_requested() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("out.png");
    let image = DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
        2,
        1,
        image::Rgba([1000, 2000, 3000, 65535]),
    ));

    save_image(&image, &path, &ExportOptions::new(OutputFormat::Png, 90)).unwrap();

    assert!(matches!(
        image::open(path).unwrap(),
        DynamicImage::ImageRgba8(_)
    ));
}

#[test]
fn refuses_to_replace_an_existing_output() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("out.png");
    std::fs::write(&path, b"existing").unwrap();

    let error = save_image(
        &test_image(),
        &path,
        &ExportOptions::new(OutputFormat::Png, 90),
    )
    .unwrap_err();

    assert!(error.to_string().contains("already exists"));
    assert_eq!(std::fs::read(path).unwrap(), b"existing");
}

#[cfg(unix)]
#[test]
fn atomic_output_uses_normal_file_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let reference = dir.path().join("reference");
    let output = dir.path().join("out.png");
    std::fs::write(&reference, b"reference").unwrap();

    save_image(
        &test_image(),
        &output,
        &ExportOptions::new(OutputFormat::Png, 90),
    )
    .unwrap();

    let reference_mode = std::fs::metadata(reference).unwrap().permissions().mode() & 0o777;
    let output_mode = std::fs::metadata(output).unwrap().permissions().mode() & 0o777;
    assert_eq!(output_mode, reference_mode);
}

#[test]
fn cancellation_after_encoding_removes_temporary_output() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let dir = tempdir().unwrap();
    let path = dir.path().join("out.png");
    let checks = AtomicUsize::new(0);

    let error = save_image_with_cancel(
        &test_image(),
        &path,
        &ExportOptions::new(OutputFormat::Png, 90),
        || checks.fetch_add(1, Ordering::Relaxed) > 0,
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "export cancelled");
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn compression_preview_labels_match_output_format() {
    assert_eq!(
        compression_preview_label_for_locale(&ExportOptions::new(OutputFormat::Jpeg, 80), "en-US"),
        "Compression preview: JPEG q80"
    );
    assert_eq!(
        compression_preview_label_for_locale(&ExportOptions::new(OutputFormat::Png, 90), "en-US"),
        "Compression preview: PNG effort 6"
    );
    assert_eq!(
        compression_preview_label_for_locale(&ExportOptions::new(OutputFormat::Tiff, 90), "en-US"),
        "Compression preview: TIFF ZIP (Balanced)"
    );

    let mut tiff = ExportOptions::new(OutputFormat::Tiff, 90);
    tiff.tiff_compression = preprint::export::TiffCompression::Lzw;
    assert_eq!(
        compression_preview_label_for_locale(&tiff, "en-US"),
        "Compression preview: TIFF LZW"
    );
}

#[test]
fn compression_preview_labels_support_german_without_global_locale() {
    assert_eq!(
        compression_preview_label_for_locale(&ExportOptions::new(OutputFormat::Jpeg, 80), "de-DE"),
        "Kompressionsvorschau: JPEG q80"
    );
    assert_eq!(
        compression_preview_label_for_locale(&ExportOptions::new(OutputFormat::Tiff, 90), "de-DE"),
        "Kompressionsvorschau: TIFF ZIP (Ausgewogen)"
    );
}

#[test]
fn png_compression_levels_produce_valid_files() {
    let dir = tempdir().unwrap();
    let image = test_image();

    for level in [1u8, 6, 9] {
        let path = dir.path().join(format!("out_{level}.png"));
        let mut options = ExportOptions::new(OutputFormat::Png, 90);
        options.png_compression = level;
        save_image(&image, &path, &options).unwrap();

        assert!(path.exists(), "level {level} should write a file");
        let decoded = image::open(&path).unwrap();
        assert_eq!(decoded.to_rgba8().as_raw(), image.to_rgba8().as_raw());
    }
}

#[test]
fn tiff_compression_methods_produce_valid_files() {
    use preprint::export::{TiffCompression, TiffDeflateLevel};

    let dir = tempdir().unwrap();
    let image = test_image();

    let cases = [
        (TiffCompression::Lzw, TiffDeflateLevel::Balanced, "lzw.tiff"),
        (
            TiffCompression::Deflate,
            TiffDeflateLevel::Fast,
            "deflate_fast.tiff",
        ),
        (
            TiffCompression::Deflate,
            TiffDeflateLevel::Best,
            "deflate_best.tiff",
        ),
    ];

    for (method, level, name) in cases {
        let path = dir.path().join(name);
        let mut options = ExportOptions::new(OutputFormat::Tiff, 90);
        options.tiff_compression = method;
        options.tiff_deflate_level = level;
        save_image(&image, &path, &options).unwrap();

        assert!(path.exists(), "{name} should be written");
        let decoded = image::open(&path).unwrap();
        assert_eq!(decoded.to_rgba8().as_raw(), image.to_rgba8().as_raw());
    }
}

#[test]
fn jpeg_compression_preview_decodes_to_same_dimensions() {
    let image = test_image();
    let preview =
        compression_preview_image(image.clone(), &ExportOptions::new(OutputFormat::Jpeg, 80))
            .unwrap();

    assert_eq!(preview.width(), image.width());
    assert_eq!(preview.height(), image.height());
}

#[test]
fn jpeg_compression_preview_clamps_quality() {
    let image = test_image();
    let preview =
        compression_preview_image(image.clone(), &ExportOptions::new(OutputFormat::Jpeg, 0))
            .unwrap();

    assert_eq!(preview.width(), image.width());
    assert_eq!(preview.height(), image.height());
}

#[test]
fn lossless_compression_preview_keeps_png_pixels() {
    let image = test_image();
    let preview =
        compression_preview_image(image.clone(), &ExportOptions::new(OutputFormat::Png, 90))
            .unwrap();

    assert_eq!(preview.to_rgba8().as_raw(), image.to_rgba8().as_raw());
}
