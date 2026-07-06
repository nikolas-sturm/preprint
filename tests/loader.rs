use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use preprint::loader::{SourceBitDepth, load_image, load_image_metadata};
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
