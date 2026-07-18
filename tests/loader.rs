use std::io::Cursor;

use image::{
    DynamicImage, ExtendedColorType, GenericImageView, ImageDecoder, ImageEncoder, ImageFormat,
    Rgba, RgbaImage,
    codecs::png::{PngDecoder, PngEncoder},
    metadata::Orientation,
};
use preprint::export::{ExportOptions, OutputFormat, save_image};
use preprint::loader::{
    SourceBitDepth, load_image, load_image_metadata, load_image_with_reservation,
};
use tempfile::tempdir;

#[test]
fn decodes_image_by_magic_bytes_when_extension_is_wrong() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wrong-extension.dat");
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 1, Rgba([10, 20, 30, 255])));
    let mut encoded = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut encoded), ImageFormat::Png)
        .unwrap();
    std::fs::write(&path, encoded).unwrap();

    let loaded = load_image(&path).unwrap();

    assert_eq!(loaded.image.width(), 2);
    assert_eq!(loaded.image.height(), 1);
    assert_eq!(loaded.bit_depth, SourceBitDepth::Eight);
}

#[test]
fn reads_metadata_by_magic_bytes_without_full_load() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wrong-extension.dat");
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(3, 2, Rgba([10, 20, 30, 255])));
    let mut encoded = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut encoded), ImageFormat::Png)
        .unwrap();
    std::fs::write(&path, encoded).unwrap();

    let metadata = load_image_metadata(&path).unwrap();

    assert_eq!(metadata.dimensions, (3, 2));
    assert_eq!(metadata.bit_depth, SourceBitDepth::Eight);
}

#[test]
fn classifies_rgba16_images_as_sixteen_bit() {
    let image = DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
        1,
        1,
        image::Rgba([1000, 2000, 3000, 65535]),
    ));

    assert_eq!(SourceBitDepth::from_image(&image), SourceBitDepth::Sixteen);
}

#[test]
fn reserves_from_decoder_metadata_before_loading_pixels() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("image.png");
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(3, 2, Rgba([10, 20, 30, 255])));
    image.save(&path).unwrap();

    let (loaded, dimensions) =
        load_image_with_reservation(&path, |metadata| Ok::<_, String>(metadata.dimensions))
            .unwrap();

    assert_eq!(dimensions, (3, 2));
    assert_eq!(loaded.image.dimensions(), (3, 2));
}

#[test]
fn reservation_can_reject_image_before_decode() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("image.png");
    DynamicImage::ImageRgba8(RgbaImage::new(1, 1))
        .save(&path)
        .unwrap();

    let error =
        load_image_with_reservation(&path, |_| Err::<(), _>("budget exceeded".into())).unwrap_err();

    assert!(error.to_string().contains("budget exceeded"));
}

#[test]
fn loads_embedded_rgb_icc_profile_with_pixels() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("profiled.png");
    let image = RgbaImage::from_pixel(2, 1, Rgba([10, 20, 30, 255]));
    let profile = lcms2::Profile::new_srgb().icc().unwrap();
    let mut encoded = Vec::new();
    let mut encoder = PngEncoder::new(&mut encoded);
    encoder.set_icc_profile(profile.clone()).unwrap();
    encoder
        .write_image(image.as_raw(), 2, 1, ExtendedColorType::Rgba8)
        .unwrap();
    std::fs::write(&path, encoded).unwrap();

    let loaded = load_image(&path).unwrap();

    assert_eq!(loaded.icc_profile.as_deref(), Some(profile.as_slice()));
}

#[test]
fn bakes_all_exif_orientations_and_reports_oriented_dimensions() {
    let dir = tempdir().unwrap();
    let source = RgbaImage::from_fn(2, 3, |x, y| {
        Rgba([(y * 2 + x) as u8, x as u8, y as u8, 255])
    });

    for exif_orientation in 1..=8 {
        let path = dir
            .path()
            .join(format!("orientation-{exif_orientation}.png"));
        let mut encoded = Vec::new();
        let mut encoder = PngEncoder::new(&mut encoded);
        encoder
            .set_exif_metadata(exif_orientation_chunk(exif_orientation))
            .unwrap();
        encoder
            .write_image(source.as_raw(), 2, 3, ExtendedColorType::Rgba8)
            .unwrap();
        std::fs::write(&path, encoded).unwrap();

        let orientation = Orientation::from_exif(exif_orientation).unwrap();
        let mut expected = DynamicImage::ImageRgba8(source.clone());
        expected.apply_orientation(orientation);
        let loaded = load_image(&path).unwrap();
        let metadata = load_image_metadata(&path).unwrap();

        assert_eq!(loaded.image.dimensions(), expected.dimensions());
        assert_eq!(loaded.image.to_rgba8(), expected.to_rgba8());
        assert_eq!(metadata.dimensions, expected.dimensions());
    }
}

#[test]
fn exported_oriented_pixels_do_not_retain_source_exif() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("oriented.png");
    let output = dir.path().join("exported.png");
    let source = RgbaImage::from_pixel(2, 3, Rgba([10, 20, 30, 255]));
    let mut encoded = Vec::new();
    let mut encoder = PngEncoder::new(&mut encoded);
    encoder
        .set_exif_metadata(exif_orientation_chunk(6))
        .unwrap();
    encoder
        .write_image(source.as_raw(), 2, 3, ExtendedColorType::Rgba8)
        .unwrap();
    std::fs::write(&input, encoded).unwrap();

    let loaded = load_image(&input).unwrap();
    save_image(
        &loaded.image,
        &output,
        &ExportOptions::new(OutputFormat::Png, 90),
    )
    .unwrap();

    let mut decoder = PngDecoder::new(std::io::BufReader::new(
        std::fs::File::open(output).unwrap(),
    ))
    .unwrap();
    assert!(decoder.exif_metadata().unwrap().is_none());
    assert_eq!(decoder.orientation().unwrap(), Orientation::NoTransforms);
}

fn exif_orientation_chunk(orientation: u8) -> Vec<u8> {
    vec![
        b'M',
        b'M',
        0,
        42,
        0,
        0,
        0,
        8,
        0,
        1,
        0x01,
        0x12,
        0,
        3,
        0,
        0,
        0,
        1,
        0,
        orientation,
        0,
        0,
        0,
        0,
        0,
        0,
    ]
}

#[test]
fn rejects_rgb_profile_when_decoded_pixels_are_grayscale() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("gray.png");
    let profile = lcms2::Profile::new_srgb().icc().unwrap();
    let mut encoded = Vec::new();
    let mut encoder = PngEncoder::new(&mut encoded);
    encoder.set_icc_profile(profile).unwrap();
    encoder
        .write_image(&[127], 1, 1, ExtendedColorType::L8)
        .unwrap();
    std::fs::write(&path, encoded).unwrap();

    let error = load_image(&path).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("incompatible with decoded pixel format")
    );
}
