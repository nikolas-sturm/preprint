use preprint::{
    app::{PreprintApp, PreviewState},
    export::OutputFormat,
    loader::SourceBitDepth,
    processing::{BorderStyle, PrintSizeMm},
};

#[test]
fn app_defaults_match_print_workflow() {
    let app = PreprintApp::default();

    assert_eq!(app.print_size(), PrintSizeMm::new(100.0, 150.0));
    assert_eq!(app.border_mm(), 5.0);
    assert_eq!(app.border_style(), BorderStyle::White);
    assert_eq!(app.output_format(), OutputFormat::Png);
    assert_eq!(app.quality(), 90);
}

#[test]
fn preview_window_defaults_to_closed_fit_mode() {
    let preview = PreviewState::default();

    assert!(!preview.open);
    assert!(preview.fit_to_window);
    assert!(!preview.fullscreen);
    assert!(!preview.rendering);
}

#[test]
fn preview_state_marks_background_rendering_as_open_and_busy() {
    let mut preview = PreviewState::default();

    preview.mark_rendering();

    assert!(preview.open);
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
fn preview_state_defaults_to_magnifier_off_with_practical_lens() {
    let preview = PreviewState::default();

    assert!(!preview.magnifier_enabled());
    assert_eq!(preview.magnifier_zoom(), 4.0);
    assert_eq!(preview.magnifier_radius(), 120.0);
    assert_eq!(preview.compression_label(), "");
}

#[test]
fn preview_state_clamps_magnifier_controls() {
    let mut preview = PreviewState::default();

    preview.set_magnifier_enabled(true);
    preview.set_magnifier_zoom(0.25);
    preview.set_magnifier_radius(10.0);
    assert!(preview.magnifier_enabled());
    assert_eq!(preview.magnifier_zoom(), 2.0);
    assert_eq!(preview.magnifier_radius(), 60.0);

    preview.set_magnifier_zoom(40.0);
    preview.set_magnifier_radius(500.0);
    assert_eq!(preview.magnifier_zoom(), 12.0);
    assert_eq!(preview.magnifier_radius(), 220.0);
}

#[test]
fn preview_state_tracks_compression_label() {
    let mut preview = PreviewState::default();

    preview.set_compression_label("Compression preview: JPEG q80");

    assert_eq!(preview.compression_label(), "Compression preview: JPEG q80");
}
