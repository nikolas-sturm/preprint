use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use preprint::loader::SourceBitDepth;
use preprint::processing::{
    BorderStyle, PrintSizeMm, ProcessingError, ProcessingOptions, ResizeMode, add_border,
    add_border_with_cancel, border_pixels, calculate_ppi, output_ppi, processing_requirements,
    target_pixel_dimensions,
};
use std::sync::atomic::{AtomicUsize, Ordering};

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
fn processing_honors_cancellation_before_work_starts() {
    let source = solid_image(64, 64, Rgba([10, 20, 30, 255]));
    let options =
        ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 1.0, BorderStyle::MirroredBlur);

    let error = add_border_with_cancel(&source, &options, || true).unwrap_err();

    assert!(matches!(error, ProcessingError::Cancelled));
}

#[test]
fn processing_honors_cancellation_inside_pixel_loops() {
    let source = solid_image(64, 64, Rgba([10, 20, 30, 255]));
    let options = ProcessingOptions::new(
        PrintSizeMm::new(25.4, 25.4),
        25.4,
        BorderStyle::MirroredBlur,
    );
    let checks = AtomicUsize::new(0);

    let error = add_border_with_cancel(&source, &options, || {
        checks.fetch_add(1, Ordering::Relaxed) >= 10
    })
    .unwrap_err();

    assert!(matches!(error, ProcessingError::Cancelled));
    assert!(checks.load(Ordering::Relaxed) > 10);
}

#[test]
fn processing_honors_cancellation_inside_wide_rows() {
    let source = solid_image(4_096, 1, Rgba([10, 20, 30, 255]));
    let options = ProcessingOptions::new(PrintSizeMm::new(104.0, 1.0), 1.0, BorderStyle::White);
    let checks = AtomicUsize::new(0);

    let error = add_border_with_cancel(&source, &options, || {
        checks.fetch_add(1, Ordering::Relaxed) >= 7
    })
    .unwrap_err();

    assert!(matches!(error, ProcessingError::Cancelled));
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

#[test]
fn zero_border_preserves_original_image_variant() {
    let source = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 1, image::Rgb([1, 2, 3])));
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 0.0, BorderStyle::White);

    let output = add_border(&source, &options).unwrap();

    assert!(matches!(output, DynamicImage::ImageRgb8(_)));
}

#[test]
fn rejects_processing_plan_over_memory_limit_before_allocating() {
    let options =
        ProcessingOptions::new(PrintSizeMm::new(1.0, 1.0), 200.0, BorderStyle::MirroredBlur);

    let error = processing_requirements(2400, 2400, SourceBitDepth::Eight, &options).unwrap_err();

    assert!(error.to_string().contains("limit is 2048 MiB"));
}

#[test]
fn budgets_float_sources_conservatively() {
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 0.0, BorderStyle::White);

    let eight_bit = processing_requirements(100, 100, SourceBitDepth::Eight, &options).unwrap();
    let float = processing_requirements(100, 100, SourceBitDepth::Other, &options).unwrap();

    assert!(float.estimated_bytes > eight_bit.estimated_bytes);
}

#[test]
fn rejects_zero_border_float_clone_over_memory_limit() {
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 0.0, BorderStyle::White);

    let error = processing_requirements(8400, 8400, SourceBitDepth::Other, &options).unwrap_err();

    assert!(error.to_string().contains("limit is 2048 MiB"));
}

#[test]
fn zero_border_float_plan_budgets_clone_without_unused_output_buffers() {
    let no_border = ProcessingOptions::new(PrintSizeMm::new(254.0, 152.4), 0.0, BorderStyle::White);
    let with_border =
        ProcessingOptions::new(PrintSizeMm::new(254.0, 152.4), 1.0, BorderStyle::White);

    assert!(processing_requirements(10_000, 6_000, SourceBitDepth::Other, &no_border).is_ok());
    let error =
        processing_requirements(10_000, 6_000, SourceBitDepth::Other, &with_border).unwrap_err();
    assert!(error.to_string().contains("limit is 2048 MiB"));
}

#[test]
fn converts_physical_size_and_ppi_to_target_pixels() {
    assert_eq!(
        target_pixel_dimensions(PrintSizeMm::new(25.4, 50.8), 300).unwrap(),
        (300, 600)
    );
}

#[test]
fn reports_output_density_for_original_and_resized_content() {
    let original = ProcessingOptions::new(PrintSizeMm::new(254.0, 127.0), 0.0, BorderStyle::White);
    assert_eq!(
        output_ppi(3_000, 2_000, &original).unwrap(),
        preprint::processing::Ppi { x: 300.0, y: 400.0 }
    );

    let resized = original.with_resizing(ResizeMode::Fit, 360);
    assert_eq!(
        output_ppi(3_000, 2_000, &resized).unwrap(),
        preprint::processing::Ppi { x: 360.0, y: 360.0 }
    );
}

#[test]
fn reports_density_from_rounded_target_pixels() {
    let options = ProcessingOptions::new(PrintSizeMm::new(100.0, 100.0), 0.0, BorderStyle::White)
        .with_resizing(ResizeMode::Fit, 300);

    let ppi = output_ppi(2_000, 2_000, &options).unwrap();
    let expected = 1_181.0 * 25.4 / 100.0;
    assert!((ppi.x - expected).abs() < 0.001);
    assert!((ppi.y - expected).abs() < 0.001);
}

#[test]
fn subpixel_physical_target_clamps_to_one_pixel() {
    assert_eq!(
        target_pixel_dimensions(PrintSizeMm::new(1.0, 1.0), 1).unwrap(),
        (1, 1)
    );
}

#[test]
fn fit_mode_pads_to_exact_target_dimensions() {
    let source = solid_image(4, 2, Rgba([10, 20, 30, 255]));
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 0.0, BorderStyle::White)
        .with_resizing(ResizeMode::Fit, 4);

    let output = add_border(&source, &options).unwrap().to_rgba8();

    assert_eq!(output.dimensions(), (4, 4));
    assert_eq!(output.get_pixel(0, 0), &Rgba([255; 4]));
    assert_eq!(output.get_pixel(0, 1), &Rgba([10, 20, 30, 255]));
}

#[test]
fn fill_mode_center_crops_to_exact_target_dimensions() {
    let mut source = RgbaImage::new(4, 2);
    for x in 0..4 {
        for y in 0..2 {
            source.put_pixel(x, y, Rgba([(x * 50) as u8, 0, 0, 255]));
        }
    }
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 0.0, BorderStyle::White)
        .with_resizing(ResizeMode::Fill, 2);

    let output = add_border(&DynamicImage::ImageRgba8(source), &options)
        .unwrap()
        .to_rgba8();

    assert_eq!(output.dimensions(), (2, 2));
    assert_eq!(output.get_pixel(0, 0).0[0], 50);
    assert_eq!(output.get_pixel(1, 0).0[0], 100);
}

#[test]
fn resized_border_is_added_outside_target_size() {
    let source = solid_image(4, 2, Rgba([10, 20, 30, 255]));
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 6.35, BorderStyle::Black)
        .with_resizing(ResizeMode::Fit, 4);

    let output = add_border(&source, &options).unwrap();

    assert_eq!(output.dimensions(), (6, 6));
}

#[test]
fn fit_mode_preserves_sixteen_bit_depth() {
    let source = DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
        4,
        2,
        image::Rgba([1000, 2000, 3000, 65535]),
    ));
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 0.0, BorderStyle::Black)
        .with_resizing(ResizeMode::Fit, 4);

    let output = add_border(&source, &options).unwrap();

    assert!(matches!(output, DynamicImage::ImageRgba16(_)));
    assert_eq!(output.dimensions(), (4, 4));
}

#[test]
fn fill_mode_rejects_extreme_resize_intermediate_before_allocating() {
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 0.0, BorderStyle::White)
        .with_resizing(ResizeMode::Fill, 1000);

    let error = processing_requirements(1, 10_000, SourceBitDepth::Eight, &options).unwrap_err();

    assert!(error.to_string().contains("limit is 2048 MiB"));
}

#[test]
fn fill_mode_budgets_lanczos_float_intermediate() {
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 0.0, BorderStyle::White)
        .with_resizing(ResizeMode::Fill, 1000);

    let error =
        processing_requirements(100_000, 1_200, SourceBitDepth::Eight, &options).unwrap_err();

    assert!(error.to_string().contains("limit is 2048 MiB"));
}

#[test]
fn float_resize_budgets_full_precision_working_buffers() {
    let options = ProcessingOptions::new(PrintSizeMm::new(203.2, 101.6), 0.0, BorderStyle::White)
        .with_resizing(ResizeMode::Fit, 1000);

    let error = processing_requirements(8_000, 4_000, SourceBitDepth::Other, &options).unwrap_err();

    assert!(error.to_string().contains("limit is 2048 MiB"));
}

#[test]
fn alpha_resize_does_not_bleed_hidden_rgb_into_visible_pixels() {
    let mut source = RgbaImage::new(2, 1);
    source.put_pixel(0, 0, Rgba([255, 0, 0, 0]));
    source.put_pixel(1, 0, Rgba([0, 0, 255, 255]));
    let options = ProcessingOptions::new(PrintSizeMm::new(25.4, 12.7), 0.0, BorderStyle::White)
        .with_resizing(ResizeMode::Fill, 4);

    let output = add_border(&DynamicImage::ImageRgba8(source), &options)
        .unwrap()
        .to_rgba8();

    for pixel in output.pixels() {
        assert_eq!(pixel.0[0], 0, "hidden red leaked into {pixel:?}");
    }
}

#[test]
fn alpha_resize_preserves_low_alpha_color_precision() {
    let source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 1, Rgba([128, 0, 0, 1])));
    let options = ProcessingOptions::new(PrintSizeMm::new(50.8, 25.4), 0.0, BorderStyle::White)
        .with_resizing(ResizeMode::Fill, 1);

    let output = add_border(&source, &options).unwrap().to_rgba8();

    assert_eq!(output.get_pixel(0, 0), &Rgba([128, 0, 0, 1]));
}

#[test]
fn mirrored_blur_does_not_bleed_hidden_rgb_into_visible_pixels() {
    let mut source = RgbaImage::new(2, 1);
    source.put_pixel(0, 0, Rgba([255, 0, 0, 0]));
    source.put_pixel(1, 0, Rgba([0, 0, 255, 255]));
    let options = ProcessingOptions::new(
        PrintSizeMm::new(25.4, 12.7),
        25.4,
        BorderStyle::MirroredBlur,
    );

    let output = add_border(&DynamicImage::ImageRgba8(source), &options)
        .unwrap()
        .to_rgba8();

    for pixel in output.pixels().filter(|pixel| pixel.0[3] > 0) {
        assert_eq!(pixel.0[0], 0, "hidden red leaked into {pixel:?}");
    }
}
