use preprint::{
    app::{PreprintApp, PreviewState},
    export::{BitDepth, OutputFormat, TiffCompression, TiffDeflateLevel},
    loader::SourceBitDepth,
    processing::{BorderStyle, PrintSizeMm},
};

#[test]
fn app_defaults_match_print_workflow() {
    let app = PreprintApp::default();

    assert_eq!(app.print_size(), PrintSizeMm::new(600.0, 400.0));
    assert_eq!(app.border_mm(), 8.0);
    assert_eq!(app.border_style(), BorderStyle::MirroredBlur);
    assert_eq!(app.output_format(), OutputFormat::Tiff);
    assert_eq!(app.bit_depth(), BitDepth::Sixteen);
    assert_eq!(app.tiff_compression(), TiffCompression::Deflate);
    assert_eq!(app.tiff_deflate_level(), TiffDeflateLevel::Best);
    assert_eq!(app.quality(), 90);
}

#[test]
fn preview_state_defaults_to_idle() {
    let preview = PreviewState::default();

    assert!(!preview.rendering);
}

#[test]
fn preview_state_marks_background_rendering_busy() {
    let mut preview = PreviewState::default();

    preview.mark_rendering();

    assert!(preview.rendering);
    assert!(preview.progress() > 0.0);
}

#[test]
fn preview_state_tracks_progress_label_and_softproof_toggle() {
    let mut preview = PreviewState::default();

    preview.set_progress(0.35, "Applying border");
    preview.set_softproof_enabled(false);

    assert_eq!(preview.progress(), 0.35);
    assert_eq!(preview.progress_label(), "Applying border");
    assert!(!preview.softproof_enabled());
}

#[test]
fn sixteen_bit_tiff_is_available_only_for_sixteen_bit_sources() {
    assert!(!PreprintApp::sixteen_bit_tiff_available(
        SourceBitDepth::Eight,
        OutputFormat::Tiff,
    ));
    assert!(PreprintApp::sixteen_bit_tiff_available(
        SourceBitDepth::Sixteen,
        OutputFormat::Tiff,
    ));
    assert!(!PreprintApp::sixteen_bit_tiff_available(
        SourceBitDepth::Sixteen,
        OutputFormat::Png,
    ));
}

#[test]
fn preview_state_defaults_to_base_view() {
    let preview = PreviewState::default();

    assert_eq!(preview.compression_label(), "");
    assert!(!preview.crop_overlay_enabled());
    assert_eq!(preview.zoom_percent(), 100);
}

#[test]
fn preview_state_tracks_crop_overlay_toggle() {
    let mut preview = PreviewState::default();

    preview.set_crop_overlay_enabled(true);

    assert!(preview.crop_overlay_enabled());
}

#[test]
fn preview_state_clamps_zoom_percentage() {
    let mut preview = PreviewState::default();

    preview.set_zoom_percent(1);
    assert_eq!(preview.zoom_percent(), 10);

    preview.set_zoom_percent(900);
    assert_eq!(preview.zoom_percent(), 800);
}

#[test]
fn preview_state_tracks_compression_label() {
    let mut preview = PreviewState::default();

    preview.set_compression_label("Compression preview: JPEG q80");

    assert_eq!(preview.compression_label(), "Compression preview: JPEG q80");
}
