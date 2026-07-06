use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use preprint::processing::{
    BorderStyle, PrintSizeMm, ProcessingOptions, add_border, border_pixels, calculate_ppi,
};

fn solid_image(width: u32, height: u32, pixel: Rgba<u8>) -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(width, height, pixel))
}

#[test]
fn calculates_ppi_from_pixels_and_millimeters() {
    let ppi = calculate_ppi(3000, 2000, PrintSizeMm::new(254.0, 127.0)).unwrap();

    assert!((ppi.x - 300.0).abs() < 0.001);
    assert!((ppi.y - 400.0).abs() < 0.001);
}

#[test]
fn rejects_zero_print_size() {
    let err = calculate_ppi(3000, 2000, PrintSizeMm::new(0.0, 127.0)).unwrap_err();

    assert!(err.to_string().contains("print size must be positive"));
}

#[test]
fn converts_border_millimeters_to_axis_pixels() {
    let ppi = calculate_ppi(3000, 2000, PrintSizeMm::new(254.0, 127.0)).unwrap();

    let border = border_pixels(2.0, ppi).unwrap();

    assert_eq!(border.x, 24);
    assert_eq!(border.y, 31);
}

#[test]
fn adds_white_border_outside_source_image() {
    let source = solid_image(2, 1, Rgba([10, 20, 30, 255]));
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 25.4, BorderStyle::White);

    let output = add_border(&source, &options).unwrap();

    assert_eq!(output.dimensions(), (6, 3));
    assert_eq!(
        output.to_rgba8().get_pixel(0, 0),
        &Rgba([255, 255, 255, 255])
    );
    assert_eq!(output.to_rgba8().get_pixel(2, 1), &Rgba([10, 20, 30, 255]));
}

#[test]
fn adds_black_border_outside_source_image() {
    let source = solid_image(2, 1, Rgba([10, 20, 30, 255]));
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 25.4, BorderStyle::Black);

    let output = add_border(&source, &options).unwrap().to_rgba8();

    assert_eq!(output.get_pixel(0, 0), &Rgba([0, 0, 0, 255]));
    assert_eq!(output.get_pixel(2, 1), &Rgba([10, 20, 30, 255]));
}

#[test]
fn mirrored_blur_border_preserves_source_center_and_expands_canvas() {
    let source = solid_image(2, 1, Rgba([100, 120, 140, 255]));
    let options = ProcessingOptions::new(
        PrintSizeMm::new(25.4, 25.4),
        25.4,
        BorderStyle::MirroredBlur,
    );

    let output = add_border(&source, &options).unwrap();

    assert_eq!(output.dimensions(), (6, 3));
    assert_eq!(
        output.to_rgba8().get_pixel(2, 1),
        &Rgba([100, 120, 140, 255])
    );
}

#[test]
fn mirrored_blur_feathers_from_unblurred_seam() {
    let mut source = RgbaImage::from_pixel(4, 1, Rgba([255, 255, 255, 255]));
    source.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
    let source = DynamicImage::ImageRgba8(source);
    let options = ProcessingOptions::new(
        PrintSizeMm::new(50.8, 25.4),
        25.4,
        BorderStyle::MirroredBlur,
    );

    let output = add_border(&source, &options).unwrap().to_rgba8();

    assert_eq!(output.dimensions(), (8, 3));
    assert!(
        output.get_pixel(1, 1).0[0] < 10,
        "seam-adjacent border should stay close to mirrored black edge, got {:?}",
        output.get_pixel(1, 1)
    );
}

#[test]
fn white_border_preserves_rgba16_source_depth() {
    let source = DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
        2,
        1,
        image::Rgba([1000, 2000, 3000, 65535]),
    ));
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 25.4, BorderStyle::White);

    let output = add_border(&source, &options).unwrap();

    let DynamicImage::ImageRgba16(output) = output else {
        panic!("expected RGBA16 output");
    };
    assert_eq!(output.dimensions(), (6, 3));
    assert_eq!(
        output.get_pixel(0, 0),
        &image::Rgba([65535, 65535, 65535, 65535])
    );
    assert_eq!(
        output.get_pixel(2, 1),
        &image::Rgba([1000, 2000, 3000, 65535])
    );
}
