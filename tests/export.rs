use image::{DynamicImage, Rgba, RgbaImage};
use preprint::{
    export::{
        BitDepth, ExportOptions, OutputFormat, can_export_bit_depth, compression_preview_image,
        compression_preview_label, extension, save_image, unique_output_path,
    },
    loader::SourceBitDepth,
};
use tempfile::tempdir;

fn test_image() -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([20, 40, 60, 255])))
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
fn compression_preview_labels_match_output_format() {
    preprint::i18n::init();
    assert_eq!(
        compression_preview_label(&ExportOptions::new(OutputFormat::Jpeg, 80)),
        "Compression preview: JPEG q80"
    );
    assert_eq!(
        compression_preview_label(&ExportOptions::new(OutputFormat::Png, 90)),
        "Compression preview: PNG effort 6"
    );
    assert_eq!(
        compression_preview_label(&ExportOptions::new(OutputFormat::Tiff, 90)),
        "Compression preview: TIFF ZIP (Balanced)"
    );

    let mut tiff = ExportOptions::new(OutputFormat::Tiff, 90);
    tiff.tiff_compression = preprint::export::TiffCompression::Lzw;
    assert_eq!(
        compression_preview_label(&tiff),
        "Compression preview: TIFF LZW"
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
