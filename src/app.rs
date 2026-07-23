use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context as _, Result};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, ExternalPaths, Focusable,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, ParentElement, Render, ScrollDelta,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled, Subscription, Window,
    div, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, IndexPath, Selectable, StyledExt, Theme, ThemeMode,
    button::{Button, ButtonVariants as _},
    input::{
        Escape as InputEscape, InputEvent, InputState, NumberInput, NumberInputEvent, SelectAll,
        StepAction,
    },
    progress::Progress,
    select::{Select, SelectEvent, SelectItem, SelectState},
    slider::{Slider, SliderEvent, SliderState},
    tooltip::Tooltip,
    v_flex,
};
use image::{DynamicImage, ImageFormat};
use lcms2::{InfoType, Locale, Profile};
use rayon::prelude::*;
use rfd::FileDialog;

use crate::updater::{self, AvailableUpdate};
use crate::{
    export::{
        BitDepth, ExportError, ExportOptions, OutputFormat, TiffCompression, TiffDeflateLevel,
        can_export_bit_depth, compression_preview_image, compression_preview_label,
        save_image_with_icc_profile_and_cancel, unique_output_path,
    },
    i18n,
    loader::{SourceBitDepth, load_image, load_image_metadata, load_image_with_reservations},
    preferences::{self, LengthUnit, ThemePreference, WorkflowPreferences},
    preview::{PreviewBitmap, print_preview_canvas},
    processing::{
        BorderStyle, CropRect, MAX_CONCURRENT_PROCESSING_BYTES, PrintSizeMm, ProcessingError,
        ProcessingOptions, add_border_with_cancel, add_preview_border_with_cancel, calculate_ppi,
        crop_rect, output_ppi, processing_requirements,
    },
    softproof::{
        SoftproofError, SoftproofSettings, apply_preview_profile_with_source,
        apply_source_profile_to_srgb, convert_to_output_profile, load_rgb_output_profile,
    },
};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "dat"];
const MAX_QUEUE_FILES: usize = 500;
const MAX_PREVIEW_DIM: u32 = 2400;
const THUMBNAIL_MAX_DIMENSION: u32 = 160;
const THUMBNAIL_DISPLAY_SIZE: f32 = 68.0;
const MIN_PREVIEW_ZOOM_PERCENT: u16 = 10;
const MAX_PREVIEW_ZOOM_PERCENT: u16 = 800;
const PREVIEW_ZOOM_STEP_PERCENT: i32 = 10;

gpui::actions!(preprint, [ZoomIn, ZoomOut]);

pub fn preview_zoom_key_bindings() -> [KeyBinding; 3] {
    [
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl-=", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
    ]
}

fn tr(key: &str) -> String {
    i18n::translate(key)
}

pub struct PreprintApp {
    files: Vec<FileEntry>,
    selected_index: usize,
    print_width_cm: f32,
    print_height_cm: f32,
    length_unit: LengthUnit,
    print_preset: PrintPreset,
    syncing_print_inputs: bool,
    border_mm: f32,
    border_style: BorderStyle,
    output_format: OutputFormat,
    quality: u8,
    bit_depth: BitDepth,
    png_compression: u8,
    tiff_compression: TiffCompression,
    tiff_deflate_level: TiffDeflateLevel,
    output_dir: Option<PathBuf>,
    softproof: SoftproofSettings,
    convert_output_profile: bool,
    pub(crate) preview: PreviewState,
    pub(crate) preview_base: Option<PreviewBitmap>,
    pub(crate) preview_softproof: Option<PreviewBitmap>,
    pub(crate) preview_crop_base: Option<PreviewBitmap>,
    pub(crate) preview_crop_softproof: Option<PreviewBitmap>,
    pub(crate) preview_crop_rect: Option<CropRect>,
    preview_image_size: Option<[usize; 2]>,
    preview_request_id: u64,
    preview_cancel: Option<Arc<AtomicBool>>,
    preview_worker_active: bool,
    preview_refresh_ready: bool,
    status_message: Option<StatusMessage>,
    batch: Option<BatchState>,
    importing: bool,
    preferences_enabled: bool,
    preferences_save_generation: u64,
    update: AppUpdateState,
    advanced_open: bool,
    editing_input_original: Option<String>,
    ui: Option<AppUiState>,
    logo: Arc<gpui::Image>,
}

impl Default for PreprintApp {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            selected_index: 0,
            print_width_cm: 60.0,
            print_height_cm: 40.0,
            length_unit: LengthUnit::Centimeters,
            print_preset: PrintPreset::Size60x40,
            syncing_print_inputs: false,
            border_mm: 8.0,
            border_style: BorderStyle::MirroredBlur,
            output_format: OutputFormat::Tiff,
            quality: 90,
            bit_depth: BitDepth::Sixteen,
            png_compression: 6,
            tiff_compression: TiffCompression::Deflate,
            tiff_deflate_level: TiffDeflateLevel::Best,
            output_dir: None,
            softproof: SoftproofSettings::default(),
            convert_output_profile: false,
            preview: PreviewState::default(),
            preview_base: None,
            preview_softproof: None,
            preview_crop_base: None,
            preview_crop_softproof: None,
            preview_crop_rect: None,
            preview_image_size: None,
            preview_request_id: 0,
            preview_cancel: None,
            preview_worker_active: false,
            preview_refresh_ready: false,
            status_message: None,
            batch: None,
            importing: false,
            preferences_enabled: false,
            preferences_save_generation: 0,
            update: AppUpdateState::Idle,
            advanced_open: false,
            editing_input_original: None,
            ui: None,
            logo: Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                include_bytes!("../assets/logo_220x220.png").to_vec(),
            )),
        }
    }
}

struct AppUiState {
    language: Entity<SelectState<Vec<LanguageOption>>>,
    print_width: Entity<InputState>,
    print_height: Entity<InputState>,
    length_unit: Entity<SelectState<Vec<LengthUnit>>>,
    print_preset: Entity<SelectState<Vec<PrintPreset>>>,
    border_width: Entity<InputState>,
    border_style: Entity<SelectState<Vec<BorderStyle>>>,
    output_format: Entity<SelectState<Vec<OutputFormat>>>,
    bit_depth: Entity<SelectState<Vec<BitDepth>>>,
    tiff_compression: Entity<SelectState<Vec<TiffCompression>>>,
    tiff_deflate_level: Entity<SelectState<Vec<TiffDeflateLevel>>>,
    quality: Entity<SliderState>,
    quality_input: Entity<InputState>,
    png_compression: Entity<SliderState>,
    png_compression_input: Entity<InputState>,
    preview_zoom: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct LanguageOption {
    id: &'static str,
    label: &'static str,
}

impl SelectItem for LanguageOption {
    type Value = &'static str;

    fn title(&self) -> SharedString {
        self.label.into()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

impl SelectItem for LengthUnit {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PrintPreset {
    #[default]
    Custom,
    Size60x40,
    Size30x20,
    A4,
    A3,
    A2,
}

impl PrintPreset {
    const ALL: [Self; 6] = [
        Self::Custom,
        Self::Size60x40,
        Self::Size30x20,
        Self::A4,
        Self::A3,
        Self::A2,
    ];

    fn label(self) -> String {
        tr(match self {
            Self::Custom => "preset-custom",
            Self::Size60x40 => "preset-60x40",
            Self::Size30x20 => "preset-30x20",
            Self::A4 => "preset-a4",
            Self::A3 => "preset-a3",
            Self::A2 => "preset-a2",
        })
    }

    fn dimensions_cm(self, landscape: bool) -> Option<(f32, f32)> {
        let (long, short) = match self {
            Self::Custom => return None,
            Self::Size60x40 => (60.0, 40.0),
            Self::Size30x20 => (30.0, 20.0),
            Self::A4 => (29.7, 21.0),
            Self::A3 => (42.0, 29.7),
            Self::A2 => (59.4, 42.0),
        };
        Some(if landscape {
            (long, short)
        } else {
            (short, long)
        })
    }

    fn matching(width_cm: f32, height_cm: f32) -> Self {
        Self::ALL
            .into_iter()
            .skip(1)
            .find(|preset| {
                preset
                    .dimensions_cm(width_cm >= height_cm)
                    .is_some_and(|(width, height)| {
                        (width_cm - width).abs() < 0.001 && (height_cm - height).abs() < 0.001
                    })
            })
            .unwrap_or(Self::Custom)
    }
}

impl SelectItem for PrintPreset {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

impl LengthUnit {
    const ALL: [Self; 3] = [Self::Millimeters, Self::Centimeters, Self::Inches];

    fn label(self) -> String {
        tr(match self {
            Self::Millimeters => "unit-millimeters",
            Self::Centimeters => "unit-centimeters",
            Self::Inches => "unit-inches",
        })
    }

    const fn suffix(self) -> &'static str {
        match self {
            Self::Millimeters => "mm",
            Self::Centimeters => "cm",
            Self::Inches => "in",
        }
    }

    fn display_value(self, value: f32) -> f32 {
        match self {
            Self::Millimeters => value * 10.0,
            Self::Centimeters => value,
            Self::Inches => value / 2.54,
        }
    }

    fn to_centimeters(self, value: f32) -> f32 {
        match self {
            Self::Millimeters => value / 10.0,
            Self::Centimeters => value,
            Self::Inches => value * 2.54,
        }
    }

    const fn step(self) -> f32 {
        match self {
            Self::Millimeters => 5.0,
            Self::Centimeters => 0.5,
            Self::Inches => 0.25,
        }
    }
}

impl SelectItem for BorderStyle {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

impl SelectItem for OutputFormat {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

impl SelectItem for BitDepth {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

impl SelectItem for TiffCompression {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

impl SelectItem for TiffDeflateLevel {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewState {
    pub rendering: bool,
    progress: f32,
    progress_label: String,
    softproof_enabled: bool,
    compression_label: String,
    crop_overlay_enabled: bool,
    zoom_percent: u16,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            rendering: false,
            progress: 0.0,
            progress_label: tr("idle"),
            softproof_enabled: true,
            compression_label: String::new(),
            crop_overlay_enabled: false,
            zoom_percent: 100,
        }
    }
}

impl PreviewState {
    pub fn mark_rendering(&mut self) {
        self.rendering = true;
        self.set_progress(0.05, tr("starting-preview"));
    }

    fn mark_finished(&mut self) {
        self.rendering = false;
        self.set_progress(1.0, tr("preview-ready"));
    }

    pub fn set_progress(&mut self, progress: f32, label: impl Into<String>) {
        self.progress = progress.clamp(0.0, 1.0);
        self.progress_label = label.into();
    }

    pub fn progress(&self) -> f32 {
        self.progress
    }

    pub fn progress_label(&self) -> &str {
        &self.progress_label
    }

    pub fn set_softproof_enabled(&mut self, enabled: bool) {
        self.softproof_enabled = enabled;
    }

    pub fn softproof_enabled(&self) -> bool {
        self.softproof_enabled
    }

    pub fn compression_label(&self) -> &str {
        &self.compression_label
    }

    pub fn set_compression_label(&mut self, label: impl Into<String>) {
        self.compression_label = label.into();
    }

    pub fn crop_overlay_enabled(&self) -> bool {
        self.crop_overlay_enabled
    }

    pub fn set_crop_overlay_enabled(&mut self, enabled: bool) {
        self.crop_overlay_enabled = enabled;
    }

    pub fn zoom_percent(&self) -> u16 {
        self.zoom_percent
    }

    pub fn set_zoom_percent(&mut self, percent: u16) {
        self.zoom_percent = percent.clamp(MIN_PREVIEW_ZOOM_PERCENT, MAX_PREVIEW_ZOOM_PERCENT);
    }
}

impl PreprintApp {
    pub fn new(
        workflow: WorkflowPreferences,
        preferences_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::default();
        this.apply_workflow_preferences(&workflow);
        this.preferences_enabled = preferences_error.is_none();
        if let Some(error) = preferences_error {
            this.status_message = Some(StatusMessage::error(
                rust_i18n::t!("preferences-load-failed", error = error).into_owned(),
            ));
        }
        let current_language = i18n::current_language();
        let language_index = i18n::LANGUAGES
            .iter()
            .position(|(id, _)| *id == current_language)
            .unwrap_or(0);
        let language = cx.new(|cx| {
            SelectState::new(
                i18n::LANGUAGES
                    .iter()
                    .map(|(id, label)| LanguageOption { id, label })
                    .collect(),
                Some(IndexPath::default().row(language_index)),
                window,
                cx,
            )
        });
        let print_width = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format_length(
                    this.length_unit.display_value(this.print_width_cm),
                ))
                .validate(|value, _| decimal_input_is_valid(value, 2000.0))
        });
        let print_height = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format_length(
                    this.length_unit.display_value(this.print_height_cm),
                ))
                .validate(|value, _| decimal_input_is_valid(value, 2000.0))
        });
        let length_unit = cx.new(|cx| {
            SelectState::new(
                LengthUnit::ALL.to_vec(),
                Some(
                    IndexPath::default().row(
                        LengthUnit::ALL
                            .iter()
                            .position(|unit| *unit == this.length_unit)
                            .unwrap_or(1),
                    ),
                ),
                window,
                cx,
            )
        });
        let print_preset = cx.new(|cx| {
            SelectState::new(
                PrintPreset::ALL.to_vec(),
                Some(
                    IndexPath::default().row(
                        PrintPreset::ALL
                            .iter()
                            .position(|preset| *preset == this.print_preset)
                            .unwrap_or(0),
                    ),
                ),
                window,
                cx,
            )
        });
        let border_width = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(this.border_mm.to_string())
                .validate(|value, _| decimal_input_is_valid(value, 200.0))
        });
        let quality_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(this.quality.to_string())
                .validate(|value, _| integer_input_is_valid(value, 100))
        });
        let png_compression_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(this.png_compression.to_string())
                .validate(|value, _| integer_input_is_valid(value, 9))
        });
        let preview_zoom = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(this.preview.zoom_percent().to_string())
                .validate(|value, _| preview_zoom_input_is_valid(value))
        });
        let border_style = cx.new(|cx| {
            SelectState::new(
                vec![
                    BorderStyle::White,
                    BorderStyle::Black,
                    BorderStyle::MirroredBlur,
                ],
                Some(IndexPath::default().row(match this.border_style {
                    BorderStyle::White => 0,
                    BorderStyle::Black => 1,
                    BorderStyle::MirroredBlur => 2,
                })),
                window,
                cx,
            )
        });
        let output_format = cx.new(|cx| {
            SelectState::new(
                OutputFormat::ALL.to_vec(),
                Some(
                    IndexPath::default().row(
                        OutputFormat::ALL
                            .iter()
                            .position(|format| *format == this.output_format)
                            .unwrap_or(2),
                    ),
                ),
                window,
                cx,
            )
        });
        let bit_depth = cx.new(|cx| {
            SelectState::new(
                vec![BitDepth::Eight, BitDepth::Sixteen],
                Some(IndexPath::default().row(usize::from(this.bit_depth == BitDepth::Sixteen))),
                window,
                cx,
            )
        });
        let tiff_compression = cx.new(|cx| {
            SelectState::new(
                TiffCompression::ALL.to_vec(),
                Some(IndexPath::default().row(match this.tiff_compression {
                    TiffCompression::Lzw => 0,
                    TiffCompression::Deflate => 1,
                })),
                window,
                cx,
            )
        });
        let tiff_deflate_level = cx.new(|cx| {
            SelectState::new(
                TiffDeflateLevel::ALL.to_vec(),
                Some(IndexPath::default().row(match this.tiff_deflate_level {
                    TiffDeflateLevel::Fast => 0,
                    TiffDeflateLevel::Balanced => 1,
                    TiffDeflateLevel::Best => 2,
                })),
                window,
                cx,
            )
        });
        let quality = cx.new(|_| {
            SliderState::new()
                .min(1.0)
                .max(100.0)
                .step(1.0)
                .default_value(this.quality as f32)
        });
        let png_compression = cx.new(|_| {
            SliderState::new()
                .min(1.0)
                .max(9.0)
                .step(1.0)
                .default_value(this.png_compression as f32)
        });

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe_in(
            &preview_zoom,
            window,
            |this, input, event: &InputEvent, window, cx| {
                let parsed = input.read(cx).value().parse::<u16>().ok();
                match event {
                    InputEvent::Focus => input
                        .focus_handle(cx)
                        .dispatch_action(&SelectAll, window, cx),
                    InputEvent::Change => {
                        if let Some(percent) = parsed.filter(|percent| {
                            (MIN_PREVIEW_ZOOM_PERCENT..=MAX_PREVIEW_ZOOM_PERCENT).contains(percent)
                        }) {
                            this.preview.set_zoom_percent(percent);
                            cx.notify();
                        }
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. }
                        if !parsed.is_some_and(|percent| {
                            (MIN_PREVIEW_ZOOM_PERCENT..=MAX_PREVIEW_ZOOM_PERCENT).contains(&percent)
                        }) =>
                    {
                        input.update(cx, |input, cx| {
                            input.set_value(this.preview.zoom_percent().to_string(), window, cx)
                        });
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {}
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &preview_zoom,
            window,
            |this, _, event: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step(action) = event;
                let direction = match action {
                    StepAction::Increment => 1,
                    StepAction::Decrement => -1,
                };
                this.change_preview_zoom(direction, window, cx);
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &language,
            window,
            |this, _, event: &SelectEvent<Vec<LanguageOption>>, _, cx| {
                let SelectEvent::Confirm(Some(language)) = event else {
                    return;
                };
                i18n::set_language(language);
                let save_error = preferences::save_language(language).err();
                if this.preview.rendering {
                    this.preview
                        .set_progress(this.preview.progress(), tr("starting-preview"));
                } else if this.preview_base.is_some() {
                    this.preview.set_progress(1.0, tr("preview-ready"));
                    this.preview
                        .set_compression_label(compression_preview_label(&this.export_options()));
                } else {
                    this.preview.set_progress(0.0, tr("idle"));
                }
                this.status_message = save_error.map(|error| {
                    StatusMessage::error(
                        rust_i18n::t!("preferences-save-failed", error = error.to_string())
                            .into_owned(),
                    )
                });
                cx.refresh_windows();
                cx.notify();
            },
        ));
        let width_for_unit = print_width.clone();
        let height_for_unit = print_height.clone();
        subscriptions.push(cx.subscribe_in(
            &length_unit,
            window,
            move |this, _, event: &SelectEvent<Vec<LengthUnit>>, window, cx| {
                let SelectEvent::Confirm(Some(unit)) = event else {
                    return;
                };
                if this.length_unit != *unit {
                    this.length_unit = *unit;
                    this.syncing_print_inputs = true;
                    width_for_unit.update(cx, |input, cx| {
                        input.set_value(
                            format_length(unit.display_value(this.print_width_cm)),
                            window,
                            cx,
                        )
                    });
                    height_for_unit.update(cx, |input, cx| {
                        input.set_value(
                            format_length(unit.display_value(this.print_height_cm)),
                            window,
                            cx,
                        )
                    });
                    this.syncing_print_inputs = false;
                    this.persist_workflow_preferences();
                    cx.notify();
                }
            },
        ));
        let width_for_preset = print_width.clone();
        let height_for_preset = print_height.clone();
        subscriptions.push(cx.subscribe_in(
            &print_preset,
            window,
            move |this, _, event: &SelectEvent<Vec<PrintPreset>>, window, cx| {
                let SelectEvent::Confirm(Some(preset)) = event else {
                    return;
                };
                this.print_preset = *preset;
                if let Some((width, height)) =
                    preset.dimensions_cm(this.print_width_cm >= this.print_height_cm)
                {
                    this.print_width_cm = width;
                    this.print_height_cm = height;
                    this.invalidate_preview_and_refresh(window, cx);
                    this.syncing_print_inputs = true;
                    width_for_preset.update(cx, |input, cx| {
                        input.set_value(
                            format_length(this.length_unit.display_value(width)),
                            window,
                            cx,
                        )
                    });
                    height_for_preset.update(cx, |input, cx| {
                        input.set_value(
                            format_length(this.length_unit.display_value(height)),
                            window,
                            cx,
                        )
                    });
                    this.syncing_print_inputs = false;
                }
                this.persist_workflow_preferences();
                cx.notify();
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &print_width,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if this.syncing_print_inputs {
                    return;
                }
                let parsed = parse_decimal(&input.read(cx).value())
                    .map(|value| this.length_unit.to_centimeters(value));
                match event {
                    InputEvent::Focus => input
                        .focus_handle(cx)
                        .dispatch_action(&SelectAll, window, cx),
                    InputEvent::Change => {
                        if let Some(value) = parsed.filter(|value| (0.1..=200.0).contains(value))
                            && (this.print_width_cm - value).abs() > 0.001
                        {
                            this.print_width_cm = value;
                            this.mark_custom_preset(window, cx);
                            this.invalidate_preview_and_refresh(window, cx);
                            cx.notify();
                        }
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. }
                        if !parsed.is_some_and(|v| (0.1..=200.0).contains(&v)) =>
                    {
                        input.update(cx, |input, cx| {
                            input.set_value(
                                format_length(this.length_unit.display_value(this.print_width_cm)),
                                window,
                                cx,
                            )
                        });
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        input.update(cx, |input, cx| {
                            input.set_value(
                                format_length(this.length_unit.display_value(this.print_width_cm)),
                                window,
                                cx,
                            )
                        });
                        this.persist_workflow_preferences();
                    }
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &print_width,
            window,
            |this, input, event: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step(action) = event;
                let current = parse_decimal(&input.read(cx).value())
                    .unwrap_or_else(|| this.length_unit.display_value(this.print_width_cm));
                let value = match action {
                    StepAction::Increment => current + this.length_unit.step(),
                    StepAction::Decrement => current - this.length_unit.step(),
                }
                .clamp(
                    this.length_unit.display_value(0.1),
                    this.length_unit.display_value(200.0),
                );
                let centimeters = this.length_unit.to_centimeters(value);
                if (this.print_width_cm - centimeters).abs() > 0.001 {
                    this.print_width_cm = centimeters;
                    this.mark_custom_preset(window, cx);
                    this.invalidate_preview_and_refresh(window, cx);
                }
                input.update(cx, |input, cx| {
                    input.set_value(format_length(value), window, cx)
                });
                this.persist_workflow_preferences();
                cx.notify();
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &print_height,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if this.syncing_print_inputs {
                    return;
                }
                let parsed = parse_decimal(&input.read(cx).value())
                    .map(|value| this.length_unit.to_centimeters(value));
                match event {
                    InputEvent::Focus => input
                        .focus_handle(cx)
                        .dispatch_action(&SelectAll, window, cx),
                    InputEvent::Change => {
                        if let Some(value) = parsed.filter(|value| (0.1..=200.0).contains(value))
                            && (this.print_height_cm - value).abs() > 0.001
                        {
                            this.print_height_cm = value;
                            this.mark_custom_preset(window, cx);
                            this.invalidate_preview_and_refresh(window, cx);
                            cx.notify();
                        }
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. }
                        if !parsed.is_some_and(|v| (0.1..=200.0).contains(&v)) =>
                    {
                        input.update(cx, |input, cx| {
                            input.set_value(
                                format_length(this.length_unit.display_value(this.print_height_cm)),
                                window,
                                cx,
                            )
                        });
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        input.update(cx, |input, cx| {
                            input.set_value(
                                format_length(this.length_unit.display_value(this.print_height_cm)),
                                window,
                                cx,
                            )
                        });
                        this.persist_workflow_preferences();
                    }
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &print_height,
            window,
            |this, input, event: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step(action) = event;
                let current = parse_decimal(&input.read(cx).value())
                    .unwrap_or_else(|| this.length_unit.display_value(this.print_height_cm));
                let value = match action {
                    StepAction::Increment => current + this.length_unit.step(),
                    StepAction::Decrement => current - this.length_unit.step(),
                }
                .clamp(
                    this.length_unit.display_value(0.1),
                    this.length_unit.display_value(200.0),
                );
                let centimeters = this.length_unit.to_centimeters(value);
                if (this.print_height_cm - centimeters).abs() > 0.001 {
                    this.print_height_cm = centimeters;
                    this.mark_custom_preset(window, cx);
                    this.invalidate_preview_and_refresh(window, cx);
                }
                input.update(cx, |input, cx| {
                    input.set_value(format_length(value), window, cx)
                });
                this.persist_workflow_preferences();
                cx.notify();
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &border_width,
            window,
            |this, input, event: &InputEvent, window, cx| {
                let parsed = parse_decimal(&input.read(cx).value());
                match event {
                    InputEvent::Focus => input
                        .focus_handle(cx)
                        .dispatch_action(&SelectAll, window, cx),
                    InputEvent::Change => {
                        if let Some(value) = parsed.filter(|value| (0.0..=200.0).contains(value))
                            && this.border_mm != value
                        {
                            this.border_mm = value;
                            this.invalidate_preview_and_refresh(window, cx);
                            cx.notify();
                        }
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. }
                        if !parsed.is_some_and(|v| (0.0..=200.0).contains(&v)) =>
                    {
                        input.update(cx, |input, cx| {
                            input.set_value(this.border_mm.to_string(), window, cx)
                        });
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        input.update(cx, |input, cx| {
                            input.set_value(format_length(this.border_mm), window, cx)
                        });
                        this.persist_workflow_preferences();
                    }
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &border_width,
            window,
            |this, input, event: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step(action) = event;
                let current = parse_decimal(&input.read(cx).value()).unwrap_or(this.border_mm);
                let value = match action {
                    StepAction::Increment => current + 0.5,
                    StepAction::Decrement => current - 0.5,
                }
                .clamp(0.0, 200.0);
                if this.border_mm != value {
                    this.border_mm = value;
                    this.invalidate_preview_and_refresh(window, cx);
                }
                input.update(cx, |input, cx| {
                    input.set_value(value.to_string(), window, cx)
                });
                this.persist_workflow_preferences();
                cx.notify();
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &border_style,
            window,
            |this, _, event: &SelectEvent<Vec<BorderStyle>>, window, cx| {
                let SelectEvent::Confirm(Some(value)) = event else {
                    return;
                };
                if this.border_style != *value {
                    this.border_style = *value;
                    this.invalidate_preview_and_refresh(window, cx);
                    this.persist_workflow_preferences();
                    cx.notify();
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &output_format,
            window,
            |this, _, event: &SelectEvent<Vec<OutputFormat>>, window, cx| {
                let SelectEvent::Confirm(Some(value)) = event else {
                    return;
                };
                if this.output_format != *value {
                    this.output_format = *value;
                    this.normalize_bit_depth_choice();
                    this.sync_bit_depth_select(window, cx);
                    this.invalidate_preview_and_refresh(window, cx);
                    this.persist_workflow_preferences();
                    cx.notify();
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &bit_depth,
            window,
            |this, _, event: &SelectEvent<Vec<BitDepth>>, window, cx| {
                let SelectEvent::Confirm(Some(value)) = event else {
                    return;
                };
                if this.bit_depth != *value {
                    this.bit_depth = *value;
                    this.normalize_bit_depth_choice();
                    this.sync_bit_depth_select(window, cx);
                    this.persist_workflow_preferences();
                    cx.notify();
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &tiff_compression,
            window,
            |this, _, event: &SelectEvent<Vec<TiffCompression>>, _, cx| {
                let SelectEvent::Confirm(Some(value)) = event else {
                    return;
                };
                if this.tiff_compression != *value {
                    this.tiff_compression = *value;
                    this.update_preview_compression_label();
                    this.persist_workflow_preferences();
                    cx.notify();
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &tiff_deflate_level,
            window,
            |this, _, event: &SelectEvent<Vec<TiffDeflateLevel>>, _, cx| {
                let SelectEvent::Confirm(Some(value)) = event else {
                    return;
                };
                if this.tiff_deflate_level != *value {
                    this.tiff_deflate_level = *value;
                    this.update_preview_compression_label();
                    this.persist_workflow_preferences();
                    cx.notify();
                }
            },
        ));
        let quality_slider = quality.clone();
        subscriptions.push(cx.subscribe_in(
            &quality_input,
            window,
            move |this, input, event: &InputEvent, window, cx| {
                let parsed = input.read(cx).value().parse::<u8>().ok();
                match event {
                    InputEvent::Focus => input
                        .focus_handle(cx)
                        .dispatch_action(&SelectAll, window, cx),
                    InputEvent::Change => {
                        if let Some(value) = parsed.filter(|value| (1..=100).contains(value))
                            && this.quality != value
                        {
                            this.quality = value;
                            this.invalidate_preview_and_refresh(window, cx);
                            quality_slider.update(cx, |slider, cx| {
                                slider.set_value(value as f32, window, cx)
                            });
                            cx.notify();
                        }
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. }
                        if !parsed.is_some_and(|value| (1..=100).contains(&value)) =>
                    {
                        input.update(cx, |input, cx| {
                            input.set_value(this.quality.to_string(), window, cx)
                        });
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        this.persist_workflow_preferences();
                    }
                }
            },
        ));
        let quality_slider = quality.clone();
        subscriptions.push(cx.subscribe_in(
            &quality_input,
            window,
            move |this, input, event: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step(action) = event;
                let value = match action {
                    StepAction::Increment => this.quality.saturating_add(1),
                    StepAction::Decrement => this.quality.saturating_sub(1),
                }
                .clamp(1, 100);
                if this.quality != value {
                    this.quality = value;
                    this.invalidate_preview_and_refresh(window, cx);
                }
                input.update(cx, |input, cx| {
                    input.set_value(value.to_string(), window, cx)
                });
                quality_slider.update(cx, |slider, cx| slider.set_value(value as f32, window, cx));
                this.persist_workflow_preferences();
                cx.notify();
            },
        ));
        let quality_input_for_slider = quality_input.clone();
        subscriptions.push(cx.subscribe_in(
            &quality,
            window,
            move |this, _, event: &SliderEvent, window, cx| {
                let SliderEvent::Change(value) = event;
                let value = value.start().round().clamp(1.0, 100.0) as u8;
                if this.quality != value {
                    this.quality = value;
                    this.invalidate_preview_and_refresh(window, cx);
                    this.schedule_workflow_preferences_save(cx);
                }
                quality_input_for_slider.update(cx, |input, cx| {
                    input.set_value(value.to_string(), window, cx)
                });
                cx.notify();
            },
        ));

        let png_slider = png_compression.clone();
        subscriptions.push(cx.subscribe_in(
            &png_compression_input,
            window,
            move |this, input, event: &InputEvent, window, cx| {
                let parsed = input.read(cx).value().parse::<u8>().ok();
                match event {
                    InputEvent::Focus => input
                        .focus_handle(cx)
                        .dispatch_action(&SelectAll, window, cx),
                    InputEvent::Change => {
                        if let Some(value) = parsed.filter(|value| (1..=9).contains(value))
                            && this.png_compression != value
                        {
                            this.png_compression = value;
                            this.update_preview_compression_label();
                            png_slider.update(cx, |slider, cx| {
                                slider.set_value(value as f32, window, cx)
                            });
                            cx.notify();
                        }
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. }
                        if !parsed.is_some_and(|value| (1..=9).contains(&value)) =>
                    {
                        input.update(cx, |input, cx| {
                            input.set_value(this.png_compression.to_string(), window, cx)
                        });
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        this.persist_workflow_preferences();
                    }
                }
            },
        ));
        let png_slider = png_compression.clone();
        subscriptions.push(cx.subscribe_in(
            &png_compression_input,
            window,
            move |this, input, event: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step(action) = event;
                let value = match action {
                    StepAction::Increment => this.png_compression.saturating_add(1),
                    StepAction::Decrement => this.png_compression.saturating_sub(1),
                }
                .clamp(1, 9);
                if this.png_compression != value {
                    this.png_compression = value;
                    this.update_preview_compression_label();
                }
                input.update(cx, |input, cx| {
                    input.set_value(value.to_string(), window, cx)
                });
                png_slider.update(cx, |slider, cx| slider.set_value(value as f32, window, cx));
                this.persist_workflow_preferences();
                cx.notify();
            },
        ));
        let png_input_for_slider = png_compression_input.clone();
        subscriptions.push(cx.subscribe_in(
            &png_compression,
            window,
            move |this, _, event: &SliderEvent, window, cx| {
                let SliderEvent::Change(value) = event;
                let value = value.start().round().clamp(1.0, 9.0) as u8;
                if this.png_compression != value {
                    this.png_compression = value;
                    this.update_preview_compression_label();
                    this.schedule_workflow_preferences_save(cx);
                }
                png_input_for_slider.update(cx, |input, cx| {
                    input.set_value(value.to_string(), window, cx)
                });
                cx.notify();
            },
        ));

        for input in [
            print_width.clone(),
            print_height.clone(),
            border_width.clone(),
            quality_input.clone(),
            png_compression_input.clone(),
            preview_zoom.clone(),
        ] {
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Focus => {
                        this.editing_input_original = Some(input.read(cx).value().to_string());
                    }
                    InputEvent::PressEnter { .. } => {
                        window.blur();
                        cx.notify();
                    }
                    InputEvent::Blur => {
                        let value = input.read(cx).value();
                        input.update(cx, |input, cx| input.set_value(value, window, cx));
                        this.editing_input_original = None;
                    }
                    InputEvent::Change => {}
                },
            ));
        }

        subscriptions.push(cx.on_release(|this, _| {
            if let Some(cancel) = this.preview_cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            if let Some(batch) = &this.batch {
                batch.cancel.store(true, Ordering::Release);
            }
            if this.preferences_enabled {
                let _ = preferences::save_workflow(this.workflow_preferences());
            }
        }));

        this.ui = Some(AppUiState {
            language,
            print_width,
            print_height,
            length_unit,
            print_preset,
            border_width,
            border_style,
            output_format,
            bit_depth,
            tiff_compression,
            tiff_deflate_level,
            quality,
            quality_input,
            png_compression,
            png_compression_input,
            preview_zoom,
            _subscriptions: subscriptions,
        });
        this
    }

    fn apply_workflow_preferences(&mut self, workflow: &WorkflowPreferences) {
        self.print_width_cm = workflow.print_width_cm;
        self.print_height_cm = workflow.print_height_cm;
        self.length_unit = workflow.length_unit;
        self.print_preset = PrintPreset::matching(self.print_width_cm, self.print_height_cm);
        self.border_mm = workflow.border_mm;
        self.border_style = match workflow.border_style.as_str() {
            "white" => BorderStyle::White,
            "black" => BorderStyle::Black,
            _ => BorderStyle::MirroredBlur,
        };
        self.output_format = match workflow.output_format.as_str() {
            "png" => OutputFormat::Png,
            "jpeg" => OutputFormat::Jpeg,
            _ => OutputFormat::Tiff,
        };
        self.quality = workflow.quality;
        self.bit_depth = if workflow.bit_depth == 16 && self.output_format == OutputFormat::Tiff {
            BitDepth::Sixteen
        } else {
            BitDepth::Eight
        };
        self.png_compression = workflow.png_compression;
        self.tiff_compression = if workflow.tiff_compression == "lzw" {
            TiffCompression::Lzw
        } else {
            TiffCompression::Deflate
        };
        self.tiff_deflate_level = match workflow.tiff_deflate_level.as_str() {
            "fast" => TiffDeflateLevel::Fast,
            "balanced" => TiffDeflateLevel::Balanced,
            _ => TiffDeflateLevel::Best,
        };
        if let Some(profile) = &workflow.softproof_profile {
            self.softproof.set_profile(profile);
        } else {
            self.softproof.clear_profile();
        }
        self.convert_output_profile =
            workflow.convert_output_profile && workflow.softproof_profile.is_some();
        self.output_dir = workflow.output_dir.clone();
    }

    fn workflow_preferences(&self) -> WorkflowPreferences {
        WorkflowPreferences {
            print_width_cm: self.print_width_cm,
            print_height_cm: self.print_height_cm,
            border_mm: self.border_mm,
            length_unit: self.length_unit,
            border_style: match self.border_style {
                BorderStyle::White => "white",
                BorderStyle::Black => "black",
                BorderStyle::MirroredBlur => "mirrored-blur",
            }
            .to_owned(),
            output_format: match self.output_format {
                OutputFormat::Png => "png",
                OutputFormat::Jpeg => "jpeg",
                OutputFormat::Tiff => "tiff",
            }
            .to_owned(),
            quality: self.quality,
            bit_depth: match self.bit_depth {
                BitDepth::Eight => 8,
                BitDepth::Sixteen => 16,
            },
            png_compression: self.png_compression,
            tiff_compression: match self.tiff_compression {
                TiffCompression::Lzw => "lzw",
                TiffCompression::Deflate => "deflate",
            }
            .to_owned(),
            tiff_deflate_level: match self.tiff_deflate_level {
                TiffDeflateLevel::Fast => "fast",
                TiffDeflateLevel::Balanced => "balanced",
                TiffDeflateLevel::Best => "best",
            }
            .to_owned(),
            softproof_profile: self.softproof.profile_path().map(Path::to_path_buf),
            convert_output_profile: self.convert_output_profile,
            output_dir: self.output_dir.clone(),
        }
    }

    fn persist_workflow_preferences(&mut self) {
        if !self.preferences_enabled {
            return;
        }
        if let Err(error) = preferences::save_workflow(self.workflow_preferences()) {
            self.status_message = Some(StatusMessage::error(
                rust_i18n::t!("preferences-save-failed", error = error.to_string()).into_owned(),
            ));
        }
    }

    fn schedule_workflow_preferences_save(&mut self, cx: &mut Context<Self>) {
        if !self.preferences_enabled {
            return;
        }
        self.preferences_save_generation = self.preferences_save_generation.wrapping_add(1);
        let generation = self.preferences_save_generation;
        cx.spawn(async move |weak, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = weak.update(cx, |this, cx| {
                if this.preferences_save_generation == generation {
                    this.persist_workflow_preferences();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn mark_custom_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.print_preset == PrintPreset::Custom {
            return;
        }
        self.print_preset = PrintPreset::Custom;
        if let Some(ui) = &self.ui {
            ui.print_preset.update(cx, |state, cx| {
                state.set_selected_value(&PrintPreset::Custom, window, cx)
            });
        }
    }

    fn swap_print_orientation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        std::mem::swap(&mut self.print_width_cm, &mut self.print_height_cm);
        self.syncing_print_inputs = true;
        if let Some(ui) = &self.ui {
            ui.print_width.update(cx, |input, cx| {
                input.set_value(
                    format_length(self.length_unit.display_value(self.print_width_cm)),
                    window,
                    cx,
                )
            });
            ui.print_height.update(cx, |input, cx| {
                input.set_value(
                    format_length(self.length_unit.display_value(self.print_height_cm)),
                    window,
                    cx,
                )
            });
        }
        self.syncing_print_inputs = false;
        self.invalidate_preview_and_refresh(window, cx);
        self.persist_workflow_preferences();
        cx.notify();
    }

    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        if !updater::is_setup_installation() || !matches!(self.update, AppUpdateState::Idle) {
            return;
        }

        self.update = AppUpdateState::Checking;
        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async { updater::check_for_update() })
                .await;
            let _ = weak.update(cx, |this, cx| {
                this.update = match result {
                    Ok(Some(update)) => AppUpdateState::Available(update),
                    Ok(None) | Err(_) => AppUpdateState::Idle,
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn start_update(&mut self, cx: &mut Context<Self>) {
        if self.batch.as_ref().is_some_and(|batch| batch.running) {
            self.status_message = Some(StatusMessage::error(tr("finish-export-before-update")));
            cx.notify();
            return;
        }
        let AppUpdateState::Available(update) = &self.update else {
            return;
        };
        let update = update.clone();
        self.status_message = Some(StatusMessage::ok(tr("opening-update-page")));
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let (result, update) = cx
                .background_executor()
                .spawn(async move {
                    let result = updater::open_release_page(&update);
                    (result, update)
                })
                .await;
            let _ = weak.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.update = AppUpdateState::Available(update);
                    this.status_message = Some(StatusMessage::ok(tr("update-page-opened")));
                    cx.notify();
                }
                Err(error) => {
                    this.update = AppUpdateState::Available(update);
                    this.status_message = Some(StatusMessage::error(format!(
                        "{}: {error}",
                        tr("update-failed")
                    )));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn print_size(&self) -> PrintSizeMm {
        PrintSizeMm::new(self.print_width_cm * 10.0, self.print_height_cm * 10.0)
    }

    pub fn border_mm(&self) -> f32 {
        self.border_mm
    }

    pub fn border_style(&self) -> BorderStyle {
        self.border_style
    }

    pub fn output_format(&self) -> OutputFormat {
        self.output_format
    }

    pub fn quality(&self) -> u8 {
        self.quality
    }

    pub fn bit_depth(&self) -> BitDepth {
        self.bit_depth
    }

    pub fn tiff_compression(&self) -> TiffCompression {
        self.tiff_compression
    }

    pub fn tiff_deflate_level(&self) -> TiffDeflateLevel {
        self.tiff_deflate_level
    }

    pub fn sixteen_bit_tiff_available(source: SourceBitDepth, format: OutputFormat) -> bool {
        format == OutputFormat::Tiff && source == SourceBitDepth::Sixteen
    }

    fn set_preview_zoom_percent(
        &mut self,
        percent: u16,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview.set_zoom_percent(percent);
        if let Some(ui) = &self.ui {
            ui.preview_zoom.update(cx, |input, cx| {
                input.set_value(self.preview.zoom_percent().to_string(), window, cx)
            });
        }
        cx.notify();
    }

    fn change_preview_zoom(&mut self, direction: i32, window: &mut Window, cx: &mut Context<Self>) {
        let percent = (i32::from(self.preview.zoom_percent())
            + direction.signum() * PREVIEW_ZOOM_STEP_PERCENT)
            .clamp(
                i32::from(MIN_PREVIEW_ZOOM_PERCENT),
                i32::from(MAX_PREVIEW_ZOOM_PERCENT),
            ) as u16;
        self.set_preview_zoom_percent(percent, window, cx);
    }

    fn zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        self.change_preview_zoom(1, window, cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        self.change_preview_zoom(-1, window, cx);
    }

    fn cancel_input(&mut self, _: &InputEscape, window: &mut Window, cx: &mut Context<Self>) {
        let Some(original) = self.editing_input_original.take() else {
            return;
        };
        let focused_input = self.ui.as_ref().and_then(|ui| {
            [
                &ui.print_width,
                &ui.print_height,
                &ui.border_width,
                &ui.quality_input,
                &ui.png_compression_input,
                &ui.preview_zoom,
            ]
            .into_iter()
            .find(|input| input.focus_handle(cx).is_focused(window))
            .cloned()
        });
        if let Some(input) = focused_input {
            input.update(cx, |input, cx| input.set_value(original, window, cx));
            window.blur();
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn processing_options(&self) -> ProcessingOptions {
        ProcessingOptions::new(self.print_size(), self.border_mm, self.border_style)
    }

    fn export_options(&self) -> ExportOptions {
        ExportOptions {
            format: self.output_format,
            quality: self.quality,
            bit_depth: self.bit_depth,
            png_compression: self.png_compression,
            tiff_compression: self.tiff_compression,
            tiff_deflate_level: self.tiff_deflate_level,
            pixel_density: None,
        }
    }

    fn selected_entry(&self) -> Option<&FileEntry> {
        self.files.get(self.selected_index)
    }

    fn selected_file(&self) -> Option<&Path> {
        self.selected_entry().map(|entry| entry.path.as_path())
    }

    fn batch_supports_sixteen_bit(&self) -> bool {
        !self.files.is_empty()
            && self.files.iter().all(|entry| {
                entry.status.as_ref().is_some_and(|status| {
                    status.error.is_none() && status.bit_depth == Some(SourceBitDepth::Sixteen)
                })
            })
    }

    fn clear_preview_result(&mut self) {
        self.preview_base = None;
        self.preview_softproof = None;
        self.preview_crop_base = None;
        self.preview_crop_softproof = None;
        self.preview_crop_rect = None;
        self.preview_image_size = None;
        self.preview.set_compression_label("");
    }

    fn update_preview_compression_label(&mut self) {
        if self.preview_base.is_some() {
            self.preview
                .set_compression_label(compression_preview_label(&self.export_options()));
        }
    }

    fn invalidate_preview(&mut self) {
        if let Some(cancel) = self.preview_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.preview_refresh_ready = false;
        self.preview_request_id = self.preview_request_id.wrapping_add(1);
        self.preview.rendering = false;
        self.preview.set_progress(0.0, tr("idle"));
        self.clear_preview_result();
    }

    fn invalidate_preview_and_refresh(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let should_refresh = self.selected_file().is_some();
        self.invalidate_preview();
        if !should_refresh {
            return;
        }

        let request_id = self.preview_request_id;
        self.preview
            .set_progress(0.0, tr("preview-refresh-pending"));
        cx.spawn(async move |weak, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let _ = weak.update(cx, |this, cx| {
                if preview_refresh_is_current(request_id, this.preview_request_id) {
                    this.preview_refresh_ready = true;
                }
                if preview_replacement_can_start(
                    this.preview_refresh_ready,
                    this.preview_worker_active,
                    this.selected_file().is_some(),
                ) {
                    this.preview_refresh_ready = false;
                    this.start_preview_render(cx);
                }
            });
        })
        .detach();
    }

    fn normalize_bit_depth_choice(&mut self) {
        let allowed = self.output_format == OutputFormat::Tiff
            && (self.files.is_empty() || self.batch_supports_sixteen_bit());
        if self.bit_depth == BitDepth::Sixteen && !allowed {
            self.bit_depth = BitDepth::Eight;
        }
    }

    fn sync_bit_depth_select(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ui) = &self.ui else {
            return;
        };
        let can_16 = self.output_format == OutputFormat::Tiff
            && (self.files.is_empty() || self.batch_supports_sixteen_bit());
        ui.bit_depth.update(cx, |state, cx| {
            state.set_items(
                if can_16 {
                    vec![BitDepth::Eight, BitDepth::Sixteen]
                } else {
                    vec![BitDepth::Eight]
                },
                window,
                cx,
            );
            state.set_selected_value(&self.bit_depth, window, cx)
        });
    }

    fn append_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if self.queue_locked() {
            self.status_message = Some(StatusMessage::error(if self.importing {
                tr("import-in-progress")
            } else {
                tr("finish-export-before-queue")
            }));
            cx.notify();
            return;
        }

        self.importing = true;
        self.status_message = Some(StatusMessage::ok(tr("scanning-images")));
        let existing = self.files.iter().map(|entry| entry.path.clone()).collect();
        let window_handle = window.window_handle();
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let prepared = cx
                .background_executor()
                .spawn(async move { prepare_import(paths, existing) })
                .await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.finish_import(prepared, window, cx);
                });
            });
        })
        .detach();
    }

    fn finish_import(
        &mut self,
        prepared: PreparedImport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.importing = false;
        let was_empty = self.files.is_empty();
        let added_count = prepared.entries.len();
        self.files.extend(prepared.entries);
        if was_empty && !self.files.is_empty() {
            self.selected_index = 0;
        }
        self.status_message = Some(StatusMessage::ok(import_summary(
            added_count,
            prepared.duplicates,
            prepared.skipped,
            prepared.limited,
        )));
        let previous_depth = self.bit_depth;
        self.normalize_bit_depth_choice();
        if was_empty || self.bit_depth != previous_depth {
            self.invalidate_preview_and_refresh(window, cx);
        }
        self.sync_bit_depth_select(window, cx);
        cx.notify();
    }

    fn queue_locked(&self) -> bool {
        self.importing || self.batch.as_ref().is_some_and(|batch| batch.running)
    }

    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(files) = FileDialog::new()
            .add_filter("Images", IMAGE_EXTENSIONS)
            .pick_files()
        {
            self.append_paths(files, window, cx);
        }
    }

    fn pick_image_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(folder) = FileDialog::new().pick_folder() {
            self.append_paths(vec![folder], window, cx);
        }
    }

    fn remove_selected_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.queue_locked() || self.files.is_empty() {
            return;
        }
        let removed = self.files.remove(self.selected_index);
        self.selected_index = self.selected_index.min(self.files.len().saturating_sub(1));
        self.invalidate_preview_and_refresh(window, cx);
        let fallback_name = tr("image-fallback-name");
        self.status_message = Some(StatusMessage::ok(
            rust_i18n::t!(
                "removed-image",
                name = removed
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&fallback_name)
            )
            .into_owned(),
        ));
        self.normalize_bit_depth_choice();
        self.sync_bit_depth_select(window, cx);
        cx.notify();
    }

    fn clear_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.queue_locked() || self.files.is_empty() {
            return;
        }
        let count = self.files.len();
        self.files.clear();
        self.selected_index = 0;
        self.invalidate_preview();
        self.status_message = Some(StatusMessage::ok(i18n::plural("cleared-images", count)));
        self.normalize_bit_depth_choice();
        self.sync_bit_depth_select(window, cx);
        cx.notify();
    }

    fn pick_output_folder(&mut self, cx: &mut Context<Self>) {
        if let Some(folder) = FileDialog::new().pick_folder() {
            self.output_dir = Some(folder);
            self.persist_workflow_preferences();
            cx.notify();
        }
    }

    fn pick_icc_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(profile) = FileDialog::new()
            .add_filter("ICC profiles", &["icc", "icm"])
            .pick_file()
        {
            self.softproof.set_profile(profile);
            self.invalidate_preview_and_refresh(window, cx);
            self.persist_workflow_preferences();
            cx.notify();
        }
    }

    fn start_preview_render(&mut self, cx: &mut Context<Self>) {
        if self.batch.as_ref().is_some_and(|batch| batch.running) {
            return;
        }
        let Some(path) = self.selected_file().map(Path::to_path_buf) else {
            self.status_message = Some(StatusMessage::error(tr("select-image-to-preview")));
            cx.notify();
            return;
        };

        if self.preview_worker_active {
            if let Some(cancel) = self.preview_cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            self.preview_request_id = self.preview_request_id.wrapping_add(1);
            self.preview_refresh_ready = true;
            self.preview.rendering = false;
            self.preview
                .set_progress(0.0, tr("preview-refresh-pending"));
            cx.notify();
            return;
        }
        if let Some(cancel) = self.preview_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.preview_cancel = Some(cancel.clone());
        self.preview_request_id = self.preview_request_id.wrapping_add(1);
        let request_id = self.preview_request_id;
        self.preview_worker_active = true;
        self.preview_refresh_ready = false;
        let processing = self.processing_options();
        let export = self.export_options();
        let mut softproof = self.softproof.clone();
        softproof.set_enabled(true);
        self.preview.mark_rendering();
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { build_preview(path, processing, export, softproof, cancel) })
                .await;

            let _ = weak.update(cx, |this, cx| {
                this.preview_worker_active = false;
                if request_id != this.preview_request_id {
                    if preview_replacement_can_start(
                        this.preview_refresh_ready,
                        this.preview_worker_active,
                        this.selected_file().is_some(),
                    ) {
                        this.preview_refresh_ready = false;
                        this.start_preview_render(cx);
                    }
                    cx.notify();
                    return;
                }
                this.preview_cancel = None;
                match result {
                    Ok(PreviewBuildOutcome::Ready(images)) => {
                        this.preview.mark_finished();
                        let size = [images.base.width as usize, images.base.height as usize];
                        this.preview_base = Some(images.base);
                        this.preview_softproof = images.softproof;
                        this.preview_crop_base = Some(images.crop_base);
                        this.preview_crop_softproof = images.crop_softproof;
                        this.preview_crop_rect = Some(images.crop_rect);
                        this.preview_image_size = Some(size);
                        this.preview
                            .set_compression_label(compression_preview_label(
                                &this.export_options(),
                            ));
                        this.status_message = Some(StatusMessage::ok(tr("preview-ready")));
                    }
                    Ok(PreviewBuildOutcome::Cancelled) => {
                        this.preview.rendering = false;
                        this.preview.set_progress(0.0, tr("idle"));
                    }
                    Err(error) => {
                        this.preview.rendering = false;
                        this.preview.set_progress(0.0, tr("idle"));
                        this.clear_preview_result();
                        this.status_message = Some(StatusMessage::error(error));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_export(&mut self, cx: &mut Context<Self>) {
        let files = self.files.iter().map(|entry| entry.path.clone()).collect();
        self.start_export_files(files, cx);
    }

    fn choose_folder_and_export(&mut self, cx: &mut Context<Self>) {
        if self.output_dir.is_none() {
            let Some(folder) = FileDialog::new().pick_folder() else {
                return;
            };
            self.output_dir = Some(folder);
            self.persist_workflow_preferences();
        }
        self.start_export(cx);
    }

    fn start_export_files(&mut self, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(output_dir) = self.output_dir.clone() else {
            self.status_message = Some(StatusMessage::error(tr("pick-output-folder")));
            cx.notify();
            return;
        };
        let plan = BatchExportPlan {
            output_dir: output_dir.clone(),
            processing: self.processing_options(),
            export: self.export_options(),
            output_profile_path: self
                .convert_output_profile
                .then(|| self.softproof.profile_path().map(Path::to_path_buf))
                .flatten(),
            output_profile: None,
        };
        let jobs = planned_jobs(&files, &output_dir, plan.export.format);
        self.start_export_jobs(jobs, plan, Vec::new(), cx);
    }

    fn start_export_jobs(
        &mut self,
        jobs: Vec<(PathBuf, PathBuf)>,
        plan: BatchExportPlan,
        retained_results: Vec<BatchFileResult>,
        cx: &mut Context<Self>,
    ) {
        if self.importing {
            self.status_message = Some(StatusMessage::error(tr("import-in-progress")));
            cx.notify();
            return;
        }
        if self.preview_worker_active {
            self.status_message = Some(StatusMessage::error(tr("preview-in-progress")));
            cx.notify();
            return;
        }
        if self.batch.as_ref().is_some_and(|batch| batch.running) {
            return;
        }
        if jobs.is_empty() {
            self.status_message = Some(StatusMessage::error(tr("add-images-to-export")));
            cx.notify();
            return;
        }

        let total = jobs.len() + retained_results.len();
        let processing = plan.processing;
        let export = plan.export;
        let worker_count = export_worker_count(total);
        let budget = Arc::new(ProcessingBudget::new(MAX_CONCURRENT_PROCESSING_BYTES));
        let cancel = Arc::new(AtomicBool::new(false));
        self.batch = Some(BatchState {
            total,
            completed: retained_results.len(),
            running: true,
            cancelling: false,
            cancel: cancel.clone(),
            plan: plan.clone(),
            results: retained_results,
        });
        self.status_message = Some(StatusMessage::ok(i18n::plural(
            "export-started",
            worker_count,
        )));
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let preflight_plan = plan.clone();
            let preflight_cancel = cancel.clone();
            let (preflight, pool) = cx
                .background_executor()
                .spawn(async move {
                    let preflight = export_jobs_preflight(jobs, &preflight_plan, &preflight_cancel);
                    let pool = if preflight.jobs.is_empty() {
                        None
                    } else {
                        rayon::ThreadPoolBuilder::new()
                            .num_threads(worker_count.max(1))
                            .build()
                            .ok()
                            .map(Arc::new)
                    };
                    (preflight, pool)
                })
                .await;
            let ExportPreflight {
                jobs,
                results: preflight_results,
                output_profile,
            } = preflight;
            let plan_output_profile = output_profile.clone();
            if weak
                .update(cx, |this, cx| {
                    if let Some(batch) = &mut this.batch {
                        batch.completed += preflight_results.len();
                        batch.results.extend(preflight_results);
                        batch.plan.output_profile = plan_output_profile;
                        cx.notify();
                    }
                })
                .is_err()
            {
                return;
            }

            let (result_tx, result_rx) = async_channel::unbounded();
            let runtime = ExportRuntime {
                processing,
                export_options: export,
                output_profile,
                pool,
                budget,
                cancel: cancel.clone(),
            };
            let worker = cx
                .background_executor()
                .spawn(async move { export_batch(jobs, runtime, result_tx) });
            while let Ok(result) = result_rx.recv().await {
                if weak
                    .update(cx, |this, cx| {
                        if let Some(batch) = &mut this.batch {
                            batch.completed += 1;
                            batch.results.push(result);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    cancel.store(true, Ordering::Release);
                    worker.await;
                    return;
                }
            }
            worker.await;
            let _ = weak.update(cx, |this, cx| {
                let completion = if let Some(batch) = &mut this.batch {
                    batch.running = false;
                    batch.cancelling = false;
                    Some(export_completion_status(&batch.results))
                } else {
                    None
                };
                this.status_message = completion;
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel_export(&mut self, cx: &mut Context<Self>) {
        let Some(batch) = &mut self.batch else {
            return;
        };
        if !batch.running || batch.cancelling {
            return;
        }
        batch.cancelling = true;
        batch.cancel.store(true, Ordering::Release);
        self.status_message = Some(StatusMessage::ok(tr("export-cancelling")));
        cx.notify();
    }

    fn retry_failed_export(&mut self, cx: &mut Context<Self>) {
        let Some((jobs, plan, retained_results)) = self.batch.as_ref().and_then(|batch| {
            if batch.running {
                return None;
            }
            Some((
                retryable_jobs(&batch.results),
                batch.plan.clone(),
                successful_results(&batch.results),
            ))
        }) else {
            return;
        };
        self.start_export_jobs(jobs, plan, retained_results, cx);
    }

    fn reveal_output_folder(&mut self, cx: &mut Context<Self>) {
        let Some(output_dir) = self
            .batch
            .as_ref()
            .map(|batch| batch.plan.output_dir.clone())
        else {
            self.status_message = Some(StatusMessage::error(tr("pick-output-folder")));
            cx.notify();
            return;
        };
        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { open_output_folder(&output_dir) })
                .await;
            if let Err(error) = result {
                let _ = weak.update(cx, |this, cx| {
                    this.status_message = Some(StatusMessage::error(
                        rust_i18n::t!("reveal-output-failed", error = error.to_string())
                            .into_owned(),
                    ));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        icon: IconName,
        cx: &Context<Self>,
        handler: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Button {
        Button::new(id)
            .label(label)
            .icon(icon)
            .on_click(cx.listener(move |this, _, window, cx| handler(this, window, cx)))
    }

    fn number_input_with_tooltip(
        &self,
        id: &'static str,
        input: NumberInput,
        value: impl Into<SharedString>,
    ) -> impl IntoElement {
        let value: SharedString = value.into();
        div()
            .id(id)
            .tooltip(move |window, cx| Tooltip::new(value.clone()).build(window, cx))
            .child(input)
    }

    fn card(
        &self,
        title: impl Into<SharedString>,
        content: impl IntoElement,
        cx: &App,
    ) -> gpui::Div {
        let title: SharedString = title.into();
        v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(div().text_sm().font_semibold().child(title))
            .child(content)
    }

    fn value_row(
        &self,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
        controls: impl IntoElement,
    ) -> impl IntoElement {
        let label: SharedString = label.into();
        let value: SharedString = value.into();
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_1()
            .child(div().text_sm().font_medium().child(label))
            .when(!value.is_empty(), |row| {
                row.child(div().w_full().truncate().text_sm().child(value))
            })
            .child(controls)
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let export_running = self.batch.as_ref().is_some_and(|batch| batch.running);
        let ui = self.ui.as_ref().expect("PreprintApp::new initializes UI");
        let mut actions = div().flex().items_center().gap_2();
        let update_control = match &self.update {
            AppUpdateState::Available(update) => Some((
                i18n::versioned("update-to-version", &update.version),
                false,
                export_running,
            )),
            AppUpdateState::Idle | AppUpdateState::Checking => None,
        };
        if let Some((label, loading, disabled)) = update_control {
            let mut update_button = self
                .button(
                    "install-update",
                    label,
                    IconName::GitHub,
                    cx,
                    |this, _, cx| this.start_update(cx),
                )
                .loading(loading)
                .disabled(disabled);
            if export_running {
                update_button = update_button.tooltip(tr("finish-export-before-update"));
            }
            actions = actions.child(update_button);
        }
        actions = actions
            .child(Select::new(&ui.language).icon(IconName::Globe).w(px(130.)))
            .child(
                Button::new("theme")
                    .icon(if cx.theme().is_dark() {
                        IconName::Sun
                    } else {
                        IconName::Moon
                    })
                    .tooltip(if cx.theme().is_dark() {
                        tr("switch-to-light")
                    } else {
                        tr("switch-to-dark")
                    })
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        let (mode, preference) = if cx.theme().is_dark() {
                            (ThemeMode::Light, ThemePreference::Light)
                        } else {
                            (ThemeMode::Dark, ThemePreference::Dark)
                        };
                        Theme::change(mode, Some(window), cx);
                        if let Err(error) = preferences::save_theme(preference) {
                            this.status_message = Some(StatusMessage::error(
                                rust_i18n::t!("preferences-save-failed", error = error.to_string())
                                    .into_owned(),
                            ));
                        }
                        cx.refresh_windows();
                        cx.notify();
                    })),
            );

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(gpui::img(self.logo.clone()).size(px(28.)))
                    .child(
                        div()
                            .child(div().text_2xl().font_bold().child("Preprint"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(tr("tagline")),
                            ),
                    ),
            )
            .child(actions)
    }

    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let queue_locked = self.queue_locked();
        let mut list = v_flex()
            .id("files-list")
            .flex_1()
            .min_h(px(0.))
            .gap_2()
            .overflow_y_scroll();
        if self.files.is_empty() {
            list = list.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_color(cx.theme().muted_foreground)
                    .child(Icon::new(IconName::Inbox).size(px(30.)))
                    .child(tr("add-or-drag")),
            );
        } else {
            for (index, entry) in self.files.iter().enumerate() {
                let name = entry
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map_or_else(|| tr("image-fallback-name"), str::to_owned);
                let selected = self.selected_index == index;
                let (primary_metadata, secondary_metadata) = entry.metadata_lines();
                let readiness = entry.print_readiness(self.print_size());
                let thumbnail_size = entry
                    .status
                    .as_ref()
                    .and_then(|status| status.dimensions)
                    .map(|dimensions| fitted_thumbnail_size(dimensions, THUMBNAIL_DISPLAY_SIZE));
                let thumbnail: AnyElement = entry
                    .status
                    .as_ref()
                    .and_then(|status| status.thumbnail.clone())
                    .map_or_else(
                        || {
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(Icon::new(IconName::File).size(px(24.)))
                                .into_any_element()
                        },
                        |thumbnail| {
                            let (width, height) = thumbnail_size
                                .unwrap_or((THUMBNAIL_DISPLAY_SIZE, THUMBNAIL_DISPLAY_SIZE));
                            gpui::img(thumbnail)
                                .w(px(width))
                                .h(px(height))
                                .into_any_element()
                        },
                    );
                let mut row = div()
                    .id(("file", index))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(THUMBNAIL_DISPLAY_SIZE))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .rounded_sm()
                            .bg(cx.theme().muted)
                            .child(thumbnail),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap_1()
                            .child(div().truncate().font_semibold().text_sm().child(name))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(primary_metadata),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(secondary_metadata),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(if readiness.warning {
                                        cx.theme().danger
                                    } else if readiness.ready {
                                        cx.theme().success
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .child(Icon::new(if readiness.warning {
                                        IconName::TriangleAlert
                                    } else if readiness.ready {
                                        IconName::CircleCheck
                                    } else {
                                        IconName::LoaderCircle
                                    }))
                                    .child(readiness.label),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected_index = index;
                        this.invalidate_preview_and_refresh(window, cx);
                        this.normalize_bit_depth_choice();
                        this.sync_bit_depth_select(window, cx);
                        cx.notify();
                    }));
                if selected {
                    row = row
                        .bg(cx.theme().list_active)
                        .border_color(cx.theme().list_active_border);
                } else {
                    row = row
                        .border_color(cx.theme().border)
                        .hover(|style| style.bg(cx.theme().list_hover));
                }
                list = list.child(row);
            }
        }

        let add = self
            .button(
                "add-images",
                tr("add-images"),
                IconName::Plus,
                cx,
                |this, window, cx| this.pick_files(window, cx),
            )
            .disabled(queue_locked);
        let add_folder = self
            .button(
                "add-folder",
                tr("add-folder"),
                IconName::FolderOpen,
                cx,
                |this, window, cx| this.pick_image_folder(window, cx),
            )
            .disabled(queue_locked);
        let remove = self
            .button(
                "remove-image",
                tr("remove-image"),
                IconName::Close,
                cx,
                |this, window, cx| this.remove_selected_file(window, cx),
            )
            .disabled(queue_locked || self.files.is_empty());
        let clear = self
            .button(
                "clear-images",
                tr("clear-all"),
                IconName::Delete,
                cx,
                |this, window, cx| this.clear_files(window, cx),
            )
            .ghost()
            .disabled(queue_locked || self.files.is_empty());
        let actions = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(add)
            .child(add_folder)
            .child(remove)
            .child(clear);
        self.card(
            tr("card-photo-queue"),
            v_flex()
                .flex_1()
                .min_h(px(0.))
                .gap_3()
                .child(actions)
                .child(list),
            cx,
        )
        .h_full()
    }

    fn render_print(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ppi = self.ppi_label();
        let ui = self.ui.as_ref().expect("PreprintApp::new initializes UI");
        let unit_suffix = self.length_unit.suffix();
        let width_value = format!("{} {unit_suffix}", ui.print_width.read(cx).value());
        let height_value = format!("{} {unit_suffix}", ui.print_height.read(cx).value());
        let border_value = format!("{} mm", ui.border_width.read(cx).value());
        let width = self.number_input_with_tooltip(
            "print-width-control",
            NumberInput::new(&ui.print_width)
                .suffix(unit_suffix)
                .w(px(156.)),
            width_value,
        );
        let height = self.number_input_with_tooltip(
            "print-height-control",
            NumberInput::new(&ui.print_height)
                .suffix(unit_suffix)
                .w(px(156.)),
            height_value,
        );
        let unit = Select::new(&ui.length_unit).w(px(180.));
        let preset = Select::new(&ui.print_preset).w(px(180.));
        let border = self.number_input_with_tooltip(
            "border-width-control",
            NumberInput::new(&ui.border_width).suffix("mm").w(px(156.)),
            border_value,
        );
        let style = Select::new(&ui.border_style).w(px(180.));
        let swap = self.button(
            "swap-orientation",
            tr("swap-orientation"),
            IconName::Redo2,
            cx,
            |this, window, cx| this.swap_print_orientation(window, cx),
        );

        let rows = v_flex()
            .gap_3()
            .child(self.value_row(tr("preset"), "", preset))
            .child(self.value_row(tr("units"), "", unit))
            .child(
                self.value_row(
                    tr("target-size"),
                    "",
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(width)
                        .child("×")
                        .child(height),
                ),
            )
            .child(self.value_row(tr("orientation"), "", swap))
            .child(self.value_row(tr("sizing"), "", div().text_sm().child(tr("crop-to-fit"))))
            .child(self.value_row(tr("border-width"), "", border))
            .child(self.value_row(tr("border-style"), "", style))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(ppi),
            );
        self.card(tr("card-print-setup"), rows, cx)
    }

    fn render_output(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = self.ui.as_ref().expect("PreprintApp::new initializes UI");
        let folder = self
            .output_dir
            .as_deref()
            .map_or_else(|| tr("no-folder"), |path| path.display().to_string());
        let folder_button = self.button(
            "folder",
            if self.output_dir.is_some() {
                tr("change")
            } else {
                tr("choose")
            },
            IconName::FolderOpen,
            cx,
            |this, _, cx| this.pick_output_folder(cx),
        );
        let format = Select::new(&ui.output_format).w(px(180.));
        let quality = div()
            .flex()
            .items_center()
            .gap_2()
            .child(Slider::new(&ui.quality).w(px(140.)))
            .child(NumberInput::new(&ui.quality_input).w(px(84.)));
        let depth = Select::new(&ui.bit_depth).w(px(180.));
        let png = div()
            .flex()
            .items_center()
            .gap_2()
            .child(Slider::new(&ui.png_compression).w(px(140.)))
            .child(
                NumberInput::new(&ui.png_compression_input)
                    .suffix("/ 9")
                    .w(px(92.)),
            );
        let tiff = Select::new(&ui.tiff_compression).w(px(180.));
        let deflate = Select::new(&ui.tiff_deflate_level).w(px(180.));
        let advanced = self
            .button(
                "toggle-advanced",
                tr("advanced-settings"),
                if self.advanced_open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                },
                cx,
                |this, _, cx| {
                    this.advanced_open = !this.advanced_open;
                    cx.notify();
                },
            )
            .ghost()
            .selected(self.advanced_open);

        let mut rows = v_flex()
            .gap_3()
            .child(self.value_row(tr("save-to"), folder, folder_button))
            .child(self.value_row(tr("format"), "", format))
            .child(advanced);
        if self.advanced_open {
            if self.output_format == OutputFormat::Jpeg {
                rows = rows.child(self.value_row(tr("quality"), "", quality));
            } else {
                rows = rows.child(self.value_row(tr("bit-depth"), "", depth));
                if self.output_format == OutputFormat::Png {
                    rows = rows.child(self.value_row(tr("compression-optimization"), "", png));
                } else {
                    rows = rows.child(self.value_row(tr("compression"), "", tiff));
                    if self.tiff_compression == TiffCompression::Deflate {
                        rows = rows.child(self.value_row(
                            tr("compression-optimization"),
                            tr("compression-optimization-help"),
                            deflate,
                        ));
                    }
                }
            }
            rows = rows.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.bit_depth_note()),
            );
        }
        self.card(tr("card-output"), rows, cx)
    }

    fn render_preview_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let profile = self.softproof.profile_path().map_or_else(
            || tr("no-softproof"),
            |path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("profile")
                    .to_owned()
            },
        );
        let choose = self.button(
            "icc",
            if self.softproof.profile_path().is_some() {
                tr("change")
            } else {
                tr("choose")
            },
            IconName::Palette,
            cx,
            |this, window, cx| this.pick_icc_profile(window, cx),
        );
        let clear = self
            .button(
                "icc-clear",
                tr("clear"),
                IconName::Close,
                cx,
                |this, window, cx| {
                    this.softproof.clear_profile();
                    this.convert_output_profile = false;
                    this.invalidate_preview_and_refresh(window, cx);
                    this.persist_workflow_preferences();
                    this.status_message = Some(StatusMessage::ok(tr("cleared-softproof")));
                    cx.notify();
                },
            )
            .disabled(self.softproof.profile_path().is_none());
        let convert_output = self
            .button(
                "convert-output-profile",
                tr("convert-output-profile"),
                IconName::Palette,
                cx,
                |this, _, cx| {
                    this.convert_output_profile = !this.convert_output_profile;
                    this.persist_workflow_preferences();
                    cx.notify();
                },
            )
            .selected(self.convert_output_profile)
            .disabled(self.softproof.profile_path().is_none());
        self.card(
            tr("card-color-management"),
            v_flex()
                .gap_3()
                .child(self.value_row(
                    tr("softproof-profile"),
                    profile,
                    div().flex().gap_1().child(choose).child(clear),
                ))
                .child(self.value_row(tr("output-profile"), "", convert_output)),
            cx,
        )
    }

    fn render_workspace_preview(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let crop_image = if self.preview.softproof_enabled() {
            self.preview_crop_softproof
                .clone()
                .or(self.preview_crop_base.clone())
        } else {
            self.preview_crop_base.clone()
        };
        let crop_overlay_available = self
            .preview_crop_rect
            .zip(crop_image.as_ref())
            .is_some_and(|(crop, image)| crop.width != image.width || crop.height != image.height);
        let image = if self.preview.crop_overlay_enabled() && crop_overlay_available {
            crop_image
        } else if self.preview.softproof_enabled() {
            self.preview_softproof.clone().or(self.preview_base.clone())
        } else {
            self.preview_base.clone()
        };
        let crop_rect = (self.preview.crop_overlay_enabled() && crop_overlay_available)
            .then_some(self.preview_crop_rect)
            .flatten();
        let softproof_available = self.preview_softproof.is_some();
        let ui = self.ui.as_ref().expect("PreprintApp::new initializes UI");
        let selected_name = self.selected_file().map_or_else(
            || tr("no-image-selected"),
            |path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Image")
                    .to_owned()
            },
        );

        let softproof = Button::new("workspace-softproof")
            .icon(if self.preview.softproof_enabled() {
                IconName::Eye
            } else {
                IconName::EyeOff
            })
            .selected(self.preview.softproof_enabled() && softproof_available)
            .disabled(!softproof_available)
            .tooltip(if softproof_available {
                tr("toggle-softproof")
            } else {
                tr("no-softproof-profile")
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.preview
                    .set_softproof_enabled(!this.preview.softproof_enabled());
                cx.notify();
            }));
        let crop = Button::new("workspace-crop")
            .icon(IconName::Frame)
            .tooltip(tr("crop-overlay"))
            .selected(self.preview.crop_overlay_enabled())
            .disabled(!crop_overlay_available)
            .on_click(cx.listener(|this, _, _, cx| {
                this.preview
                    .set_crop_overlay_enabled(!this.preview.crop_overlay_enabled());
                cx.notify();
            }));
        let refresh = Button::new("workspace-refresh")
            .icon(IconName::Redo2)
            .tooltip(tr("refresh-preview"))
            .disabled(
                self.selected_file().is_none()
                    || self.preview_worker_active
                    || self.batch.as_ref().is_some_and(|batch| batch.running),
            )
            .loading(self.preview_worker_active)
            .on_click(cx.listener(|this, _, _, cx| this.start_preview_render(cx)));
        let zoom_input = NumberInput::new(&ui.preview_zoom).suffix("%").w(px(140.));
        let is_fullscreen = window.is_fullscreen();
        let fullscreen = Button::new("workspace-fullscreen")
            .icon(if is_fullscreen {
                IconName::Minimize
            } else {
                IconName::Maximize
            })
            .tooltip(if is_fullscreen {
                tr("exit-fullscreen")
            } else {
                tr("fullscreen")
            })
            .ghost()
            .on_click(cx.listener(|_, _, window, cx| {
                window.toggle_fullscreen();
                cx.notify();
            }));

        let content: AnyElement = if self.preview.rendering {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    Progress::new()
                        .w(px(280.))
                        .value(self.preview.progress() * 100.0),
                )
                .child(self.preview.progress_label().to_owned())
                .into_any_element()
        } else if let Some(image) = image {
            div()
                .id("preview-stage")
                .flex_1()
                .min_h(px(0.))
                .p_5()
                .overflow_hidden()
                .bg(cx.theme().muted)
                .child(print_preview_canvas(
                    image,
                    crop_rect,
                    f32::from(self.preview.zoom_percent()) / 100.0,
                ))
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                    let direction = scroll_zoom_direction(event.delta);
                    if direction != 0 {
                        cx.stop_propagation();
                        this.change_preview_zoom(direction, window, cx);
                    }
                }))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(IconName::Frame).size(px(36.)))
                .child(if self.files.is_empty() {
                    tr("add-images-for-preview")
                } else {
                    tr("preparing-live-preview")
                })
                .into_any_element()
        };

        v_flex()
            .size_full()
            .min_h(px(0.))
            .overflow_hidden()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .p_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        v_flex()
                            .min_w(px(0.))
                            .child(div().font_semibold().child(tr("live-print-preview")))
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(selected_name),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(softproof)
                            .child(crop)
                            .child(refresh)
                            .child(zoom_input)
                            .child(fullscreen),
                    ),
            )
            .child(content)
            .child(
                div()
                    .p_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.ppi_label()),
                    ),
            )
    }

    fn render_inspector_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                v_flex()
                    .child(div().font_semibold().child(tr("settings")))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr("settings-apply-batch")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().secondary)
                    .text_sm()
                    .font_medium()
                    .child(Icon::new(IconName::GalleryVerticalEnd))
                    .child(rust_i18n::t!("all-photos", count = self.files.len()).into_owned()),
            )
    }

    fn render_export_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let export_running = self.batch.as_ref().is_some_and(|batch| batch.running);
        let disabled =
            self.files.is_empty() || export_running || self.preview_worker_active || self.importing;
        let blocker = if self.preview_worker_active {
            Some(tr("preview-in-progress"))
        } else if self.importing {
            Some(tr("import-in-progress"))
        } else if self.files.is_empty() {
            Some(tr("add-images-to-export"))
        } else if self.output_dir.is_none() {
            Some(tr("choose-folder-to-export"))
        } else {
            None
        };
        let (ready, warnings) = self.readiness_counts();
        let summary = rust_i18n::t!(
            "workflow-summary",
            total = self.files.len(),
            ready = ready,
            warnings = warnings
        )
        .into_owned();
        let label = if export_running {
            tr("exporting")
        } else if self.output_dir.is_none() && !self.files.is_empty() {
            tr("choose-folder-and-export")
        } else {
            rust_i18n::t!("export-photos", count = self.files.len()).into_owned()
        };
        let mut action = self
            .button(
                "export",
                label,
                IconName::ExternalLink,
                cx,
                |this, _, cx| this.choose_folder_and_export(cx),
            )
            .primary()
            .disabled(disabled)
            .loading(export_running);
        if let Some(reason) = blocker.clone() {
            action = action.tooltip(reason);
        }

        div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .py_3()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(
                v_flex()
                    .child(div().text_sm().font_semibold().child(summary))
                    .when_some(blocker, |view, reason| {
                        view.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(reason),
                        )
                    }),
            )
            .child(action)
    }

    fn render_export_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(batch) = &self.batch else {
            return self
                .card(
                    tr("card-export-status"),
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr("no-export-running")),
                    cx,
                )
                .into_any_element();
        };
        let percent = if batch.total == 0 {
            0.0
        } else {
            batch.completed as f32 * 100.0 / batch.total as f32
        };
        let mut results = v_flex()
            .id("export-results")
            .gap_1()
            .max_h(px(150.))
            .overflow_y_scroll();
        for result in &batch.results {
            let (icon, text, color) = if result.cancelled {
                (
                    IconName::Close,
                    rust_i18n::t!(
                        "export-file-cancelled",
                        name = result.input.display().to_string()
                    )
                    .into_owned(),
                    cx.theme().muted_foreground,
                )
            } else {
                match &result.error {
                    Some(error) => (
                        IconName::TriangleAlert,
                        format!("{}: {error}", result.input.display()),
                        cx.theme().danger,
                    ),
                    None => (
                        IconName::CircleCheck,
                        result
                            .output
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_default(),
                        cx.theme().success,
                    ),
                }
            };
            results = results.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(color)
                    .child(Icon::new(icon))
                    .child(text),
            );
        }
        let mut content = v_flex()
            .gap_2()
            .child(Progress::new().value(percent))
            .child(
                div()
                    .text_sm()
                    .child(format!("{} / {}", batch.completed, batch.total)),
            )
            .child(results);
        if batch.running {
            content = content.child(
                self.button(
                    "cancel-export",
                    if batch.cancelling {
                        tr("export-cancelling")
                    } else {
                        tr("cancel-export")
                    },
                    IconName::Close,
                    cx,
                    |this, _, cx| this.cancel_export(cx),
                )
                .disabled(batch.cancelling)
                .loading(batch.cancelling),
            );
        } else {
            let mut actions = div().flex().flex_wrap().gap_2();
            let recovery_disabled = self.importing;
            if batch
                .results
                .iter()
                .any(|result| result.cancelled || result.error.is_some())
            {
                actions = actions.child(
                    self.button(
                        "retry-export",
                        tr("retry-failed-export"),
                        IconName::Redo2,
                        cx,
                        |this, _, cx| this.retry_failed_export(cx),
                    )
                    .disabled(recovery_disabled),
                );
            }
            actions = actions.child(self.button(
                "reveal-output",
                tr("reveal-output-folder"),
                IconName::FolderOpen,
                cx,
                |this, _, cx| this.reveal_output_folder(cx),
            ));
            content = content.child(actions);
        }
        self.card(tr("card-export-status"), content, cx)
            .into_any_element()
    }

    fn ppi_label(&self) -> String {
        let Some(entry) = self.selected_entry() else {
            return tr("ppi-select-image");
        };
        let Some(status) = &entry.status else {
            return tr("ppi-inspecting");
        };
        if let Some(error) = &status.error {
            return error.clone();
        }
        let Some((width, height)) = status.dimensions else {
            return tr("ppi-dimensions-unavailable");
        };
        match crop_rect(width, height, self.print_size()).and_then(|crop| {
            calculate_ppi(crop.width, crop.height, self.print_size()).map(|ppi| (crop, ppi))
        }) {
            Ok((crop, ppi)) => {
                let quality = if ppi.x.min(ppi.y) < 150.0 {
                    tr("ppi-quality-low")
                } else if ppi.x.min(ppi.y) < 300.0 {
                    tr("ppi-quality-ok")
                } else {
                    tr("ppi-quality-sharp")
                };
                let note = if crop.width != width || crop.height != height {
                    tr("ppi-crop-note")
                } else {
                    String::new()
                };
                rust_i18n::t!(
                    "ppi-cropped-output",
                    width = crop.width,
                    height = crop.height,
                    ppi_x = format!("{:.0}", ppi.x),
                    ppi_y = format!("{:.0}", ppi.y),
                    quality = quality,
                    note = note
                )
                .into_owned()
            }
            Err(error) => error.to_string(),
        }
    }

    fn readiness_counts(&self) -> (usize, usize) {
        self.files.iter().fold((0, 0), |(ready, warnings), entry| {
            let readiness = entry.print_readiness(self.print_size());
            (
                ready + usize::from(readiness.ready),
                warnings + usize::from(readiness.warning),
            )
        })
    }

    fn bit_depth_note(&self) -> String {
        let key = match self.output_format {
            OutputFormat::Tiff if self.files.is_empty() => "note-16-tiff-needs-source",
            OutputFormat::Tiff if self.batch_supports_sixteen_bit() => "note-16-tiff-available",
            OutputFormat::Tiff => "note-16-tiff-8-source",
            _ => "note-16-tiff-only",
        };
        tr(key)
    }
}

impl Render for PreprintApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.is_fullscreen() {
            return div()
                .id("preprint-root")
                .on_action(cx.listener(Self::zoom_in))
                .on_action(cx.listener(Self::zoom_out))
                .on_action(cx.listener(Self::cancel_input))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, window, cx| {
                        if !window.default_prevented() {
                            window.blur();
                            cx.notify();
                        }
                    }),
                )
                .size_full()
                .overflow_hidden()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(self.render_workspace_preview(window, cx));
        }

        let (ready, warnings) = self.readiness_counts();
        let status: AnyElement = self.status_message.as_ref().map_or_else(
            || {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        rust_i18n::t!(
                            "workflow-summary",
                            total = self.files.len(),
                            ready = ready,
                            warnings = warnings
                        )
                        .into_owned(),
                    )
                    .into_any_element()
            },
            |message| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(if message.is_error {
                        cx.theme().danger
                    } else {
                        cx.theme().success
                    })
                    .child(Icon::new(if message.is_error {
                        IconName::TriangleAlert
                    } else {
                        IconName::CircleCheck
                    }))
                    .child(message.text.clone())
                    .into_any_element()
            },
        );

        div()
            .id("preprint-root")
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::cancel_input))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, window, cx| {
                    if !window.default_prevented() {
                        window.blur();
                        cx.notify();
                    }
                }),
            )
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.append_paths(paths.paths().to_vec(), window, cx)
            }))
            .child(self.render_header(cx))
            .child(status)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .w(px(300.))
                            .flex_none()
                            .h_full()
                            .min_h(px(0.))
                            .child(self.render_files(cx)),
                    )
                    .child(
                        v_flex()
                            .id("preview-workspace")
                            .flex_1()
                            .h_full()
                            .min_h(px(0.))
                            .min_w(px(320.))
                            .child(self.render_workspace_preview(window, cx)),
                    )
                    .child(
                        v_flex()
                            .id("settings-column")
                            .w(px(380.))
                            .flex_none()
                            .h_full()
                            .min_h(px(0.))
                            .gap_3()
                            .overflow_y_scroll()
                            .child(self.render_inspector_header(cx))
                            .child(self.render_print(cx))
                            .child(self.render_output(cx))
                            .child(self.render_preview_card(cx)),
                    ),
            )
            .when(self.batch.is_some(), |root| {
                root.child(
                    div()
                        .w_full()
                        .flex_none()
                        .child(self.render_export_status(cx)),
                )
            })
            .child(self.render_export_bar(cx))
    }
}

#[derive(Clone, Debug)]
struct FileEntry {
    path: PathBuf,
    status: Option<FileStatus>,
}

impl FileEntry {
    fn metadata_lines(&self) -> (String, String) {
        let Some(status) = &self.status else {
            return (tr("entry-inspecting"), String::new());
        };
        if let Some(error) = &status.error {
            return (
                rust_i18n::t!("entry-error", error = error.as_str()).into_owned(),
                String::new(),
            );
        }
        let format = status
            .format
            .map(image_format_label)
            .unwrap_or_else(|| tr("entry-format-unknown"));
        let primary = status.dimensions.map_or_else(
            || format.clone(),
            |(width, height)| {
                let megapixels = f64::from(width) * f64::from(height) / 1_000_000.0;
                format!(
                    "{format} · {} · {width} × {height} px · {megapixels:.1} MP",
                    aspect_ratio_label(width, height)
                )
            },
        );
        let secondary = match (&status.color_space, status.bit_depth) {
            (Some(color_space), Some(depth)) => format!("{color_space} · {}", depth.label()),
            (Some(color_space), None) => color_space.clone(),
            (None, Some(depth)) => depth.label(),
            (None, None) => tr("entry-ready"),
        };
        (primary, secondary)
    }

    fn print_readiness(&self, print_size: PrintSizeMm) -> PrintReadiness {
        let Some(status) = &self.status else {
            return PrintReadiness::pending(tr("entry-inspecting"));
        };
        if status.error.is_some() {
            return PrintReadiness::warning(tr("entry-needs-attention"));
        }
        let Some((width, height)) = status.dimensions else {
            return PrintReadiness::warning(tr("ppi-dimensions-unavailable"));
        };
        let Ok(crop) = crop_rect(width, height, print_size) else {
            return PrintReadiness::warning(tr("entry-needs-attention"));
        };
        let Ok(ppi) = calculate_ppi(crop.width, crop.height, print_size) else {
            return PrintReadiness::warning(tr("entry-needs-attention"));
        };
        let ppi = ppi.x.min(ppi.y).round();
        let (quality, warning) = if ppi < 150.0 {
            (tr("quality-low"), true)
        } else if ppi < 300.0 {
            (tr("quality-good"), false)
        } else {
            (tr("quality-excellent"), false)
        };
        let cropped = crop.width != width || crop.height != height;
        let label = if cropped {
            rust_i18n::t!(
                "entry-print-status-cropped",
                ppi = format!("{ppi:.0}"),
                quality = quality
            )
            .into_owned()
        } else {
            rust_i18n::t!(
                "entry-print-status",
                ppi = format!("{ppi:.0}"),
                quality = quality
            )
            .into_owned()
        };
        if warning {
            PrintReadiness::warning(label)
        } else {
            PrintReadiness::ready(label)
        }
    }
}

struct PrintReadiness {
    label: String,
    ready: bool,
    warning: bool,
}

impl PrintReadiness {
    fn ready(label: String) -> Self {
        Self {
            label,
            ready: true,
            warning: false,
        }
    }

    fn warning(label: String) -> Self {
        Self {
            label,
            ready: false,
            warning: true,
        }
    }

    fn pending(label: String) -> Self {
        Self {
            label,
            ready: false,
            warning: false,
        }
    }
}

#[derive(Clone, Debug)]
struct FileStatus {
    dimensions: Option<(u32, u32)>,
    bit_depth: Option<SourceBitDepth>,
    format: Option<ImageFormat>,
    color_space: Option<String>,
    thumbnail: Option<Arc<gpui::Image>>,
    error: Option<String>,
}

enum AppUpdateState {
    Idle,
    Checking,
    Available(AvailableUpdate),
}

#[derive(Clone, Debug)]
struct StatusMessage {
    text: String,
    is_error: bool,
}

impl StatusMessage {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }
    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

struct PreviewImages {
    base: PreviewBitmap,
    softproof: Option<PreviewBitmap>,
    crop_base: PreviewBitmap,
    crop_softproof: Option<PreviewBitmap>,
    crop_rect: CropRect,
}

enum PreviewBuildOutcome {
    Ready(PreviewImages),
    Cancelled,
}

struct BatchState {
    total: usize,
    completed: usize,
    running: bool,
    cancelling: bool,
    cancel: Arc<AtomicBool>,
    plan: BatchExportPlan,
    results: Vec<BatchFileResult>,
}

#[derive(Clone)]
struct BatchExportPlan {
    output_dir: PathBuf,
    processing: ProcessingOptions,
    export: ExportOptions,
    output_profile_path: Option<PathBuf>,
    output_profile: Option<Arc<[u8]>>,
}

struct ExportPreflight {
    jobs: Vec<(PathBuf, PathBuf)>,
    results: Vec<BatchFileResult>,
    output_profile: Option<Arc<[u8]>>,
}

struct ExportRuntime {
    processing: ProcessingOptions,
    export_options: ExportOptions,
    output_profile: Option<Arc<[u8]>>,
    pool: Option<Arc<rayon::ThreadPool>>,
    budget: Arc<ProcessingBudget>,
    cancel: Arc<AtomicBool>,
}

struct ProcessingBudget {
    limit: u64,
    in_use: Mutex<u64>,
    available: Condvar,
}

impl ProcessingBudget {
    fn new(limit: u64) -> Self {
        Self {
            limit,
            in_use: Mutex::new(0),
            available: Condvar::new(),
        }
    }

    fn acquire(
        self: &Arc<Self>,
        bytes: u64,
        cancel: Option<&AtomicBool>,
    ) -> Result<ProcessingPermit, String> {
        if bytes > self.limit {
            return Err(format!(
                "processing requires {bytes} bytes; concurrent limit is {} bytes",
                self.limit
            ));
        }

        let mut in_use = self
            .in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while self.limit.saturating_sub(*in_use) < bytes {
            if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
                return Err("export cancelled".to_owned());
            }
            let (guard, _) = self
                .available
                .wait_timeout(in_use, Duration::from_millis(50))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            in_use = guard;
        }
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
            return Err("export cancelled".to_owned());
        }
        *in_use += bytes;
        Ok(ProcessingPermit {
            budget: self.clone(),
            bytes,
        })
    }

    #[cfg(test)]
    fn in_use(&self) -> u64 {
        *self
            .in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct ProcessingPermit {
    budget: Arc<ProcessingBudget>,
    bytes: u64,
}

impl Drop for ProcessingPermit {
    fn drop(&mut self) {
        let mut in_use = self
            .budget
            .in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *in_use = in_use.saturating_sub(self.bytes);
        self.budget.available.notify_all();
    }
}

#[derive(Clone, Debug)]
struct BatchFileResult {
    input: PathBuf,
    planned_output: PathBuf,
    output: Option<PathBuf>,
    error: Option<String>,
    cancelled: bool,
}

fn decimal_input_is_valid(value: &str, max: f32) -> bool {
    if value.is_empty() || value == "." || value == "," {
        return true;
    }
    if value
        .chars()
        .filter(|character| matches!(character, '.' | ','))
        .count()
        > 1
        || !value
            .chars()
            .all(|character| character.is_ascii_digit() || matches!(character, '.' | ','))
    {
        return false;
    }
    parse_decimal(value).is_some_and(|value| value.is_finite() && value <= max)
}

fn parse_decimal(value: &str) -> Option<f32> {
    value.replace(',', ".").parse().ok()
}

fn format_length(value: f32) -> String {
    let formatted = format!("{value:.4}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

fn integer_input_is_valid(value: &str, max: u8) -> bool {
    value.is_empty()
        || (value.chars().all(|character| character.is_ascii_digit())
            && value
                .parse::<u16>()
                .is_ok_and(|value| value <= u16::from(max)))
}

fn preview_zoom_input_is_valid(value: &str) -> bool {
    value.is_empty()
        || (value.chars().all(|character| character.is_ascii_digit())
            && value
                .parse::<u16>()
                .is_ok_and(|percent| percent <= MAX_PREVIEW_ZOOM_PERCENT))
}

fn scroll_zoom_direction(delta: ScrollDelta) -> i32 {
    let vertical = match delta {
        ScrollDelta::Pixels(delta) => f32::from(delta.y),
        ScrollDelta::Lines(delta) => delta.y,
    };
    match vertical.total_cmp(&0.0) {
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
    }
}

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| IMAGE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[derive(Default)]
struct ImportDiscovery {
    images: Vec<PathBuf>,
    skipped: usize,
}

struct PreparedImport {
    entries: Vec<FileEntry>,
    duplicates: usize,
    skipped: usize,
    limited: usize,
}

fn prepare_import(paths: Vec<PathBuf>, existing: Vec<PathBuf>) -> PreparedImport {
    let discovery = discover_import_paths(paths);
    let (new_paths, duplicates) =
        deduplicate_import_paths(existing.iter().map(PathBuf::as_path), discovery.images);
    let capacity = MAX_QUEUE_FILES.saturating_sub(existing.len());
    let mut entries = Vec::new();
    let mut skipped = discovery.skipped;
    let mut limited = 0;
    for path in new_paths {
        if entries.len() >= capacity {
            limited += 1;
            continue;
        }
        let status = inspect_file(&path);
        if status.error.is_some() {
            skipped += 1;
        } else {
            entries.push(FileEntry {
                status: Some(status),
                path,
            });
        }
    }
    PreparedImport {
        entries,
        duplicates,
        skipped,
        limited,
    }
}

fn discover_import_paths(paths: Vec<PathBuf>) -> ImportDiscovery {
    let mut discovery = ImportDiscovery::default();
    let mut pending = paths;
    let mut visited_directories = HashSet::new();
    while let Some(path) = pending.pop() {
        let Ok(metadata) = std::fs::metadata(&path) else {
            discovery.skipped += 1;
            continue;
        };
        if metadata.is_dir() {
            let directory_key = import_path_key(&path);
            if !visited_directories.insert(directory_key) {
                continue;
            }
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let mut children = Vec::new();
                    for entry in entries {
                        match entry {
                            Ok(entry) => children.push(entry.path()),
                            Err(_) => discovery.skipped += 1,
                        }
                    }
                    children.sort();
                    pending.extend(children.into_iter().rev());
                }
                Err(_) => discovery.skipped += 1,
            }
        } else if metadata.is_file() && is_image_path(&path) {
            discovery.images.push(path);
        } else {
            discovery.skipped += 1;
        }
    }
    discovery.images.sort();
    discovery
}

fn deduplicate_import_paths<'a>(
    existing: impl Iterator<Item = &'a Path>,
    candidates: Vec<PathBuf>,
) -> (Vec<PathBuf>, usize) {
    let mut known: HashSet<_> = existing.map(import_path_key).collect();
    let mut unique = Vec::new();
    let mut duplicates = 0;
    for path in candidates {
        if known.insert(import_path_key(&path)) {
            unique.push(path);
        } else {
            duplicates += 1;
        }
    }
    (unique, duplicates)
}

fn import_path_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn import_summary(added: usize, duplicates: usize, skipped: usize, limited: usize) -> String {
    let mut parts = vec![i18n::plural("images-added", added)];
    if duplicates > 0 {
        parts.push(i18n::plural("duplicates-skipped", duplicates));
    }
    if skipped > 0 {
        parts.push(i18n::plural("files-skipped", skipped));
    }
    if limited > 0 {
        let key = if limited == 1 {
            "queue-limit-skipped-singular"
        } else {
            "queue-limit-skipped-other"
        };
        parts.push(rust_i18n::t!(key, count = limited, limit = MAX_QUEUE_FILES).into_owned());
    }
    parts.join(" ")
}

fn inspect_file(path: &Path) -> FileStatus {
    match load_image(path).and_then(|loaded| {
        let dimensions = (loaded.image.width(), loaded.image.height());
        let color_space = source_color_space(&loaded.image, loaded.icc_profile.as_deref());
        let thumbnail = build_file_thumbnail(&loaded.image, loaded.icc_profile.as_deref())
            .map_err(|error| crate::loader::LoadImageError::Rejected {
                path: path.to_path_buf(),
                reason: format!("failed to build thumbnail: {error}"),
            })?;
        Ok((loaded, dimensions, color_space, thumbnail))
    }) {
        Ok((loaded, dimensions, color_space, thumbnail)) => FileStatus {
            dimensions: Some(dimensions),
            bit_depth: Some(loaded.bit_depth),
            format: loaded.format,
            color_space: Some(color_space),
            thumbnail: Some(thumbnail),
            error: None,
        },
        Err(error) => FileStatus {
            dimensions: None,
            bit_depth: None,
            format: None,
            color_space: None,
            thumbnail: None,
            error: Some(error.to_string()),
        },
    }
}

fn build_file_thumbnail(
    image: &DynamicImage,
    source_icc_profile: Option<&[u8]>,
) -> Result<Arc<gpui::Image>> {
    let thumbnail = image.thumbnail(THUMBNAIL_MAX_DIMENSION, THUMBNAIL_MAX_DIMENSION);
    let thumbnail = if source_icc_profile.is_some() {
        apply_source_profile_to_srgb(&thumbnail, source_icc_profile)
            .context("failed to color-manage thumbnail")?
    } else {
        thumbnail
    };
    let thumbnail = DynamicImage::ImageRgba8(thumbnail.to_rgba8());
    let mut encoded = io::Cursor::new(Vec::new());
    thumbnail
        .write_to(&mut encoded, ImageFormat::Png)
        .context("failed to encode thumbnail")?;
    Ok(Arc::new(gpui::Image::from_bytes(
        gpui::ImageFormat::Png,
        encoded.into_inner(),
    )))
}

fn source_color_space(image: &DynamicImage, source_icc_profile: Option<&[u8]>) -> String {
    if !image.color().has_color() {
        return tr("entry-grayscale");
    }
    let Some(profile) = source_icc_profile else {
        return tr("entry-srgb-assumed");
    };
    Profile::new_icc(profile)
        .ok()
        .and_then(|profile| profile.info(InfoType::Description, Locale::none()))
        .filter(|description| !description.trim().is_empty())
        .unwrap_or_else(|| tr("entry-embedded-rgb"))
}

fn image_format_label(format: ImageFormat) -> String {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Pnm => "PNM",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Tga => "TGA",
        ImageFormat::Dds => "DDS",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Ico => "ICO",
        ImageFormat::Hdr => "HDR",
        ImageFormat::OpenExr => "OpenEXR",
        ImageFormat::Farbfeld => "farbfeld",
        ImageFormat::Avif => "AVIF",
        ImageFormat::Qoi => "QOI",
        _ => return tr("entry-format-unknown"),
    }
    .to_owned()
}

fn aspect_ratio_label(width: u32, height: u32) -> String {
    if width == 0 || height == 0 {
        return "-".to_owned();
    }
    const COMMON_RATIOS: &[(u32, u32)] = &[
        (1, 1),
        (5, 4),
        (4, 3),
        (7, 5),
        (3, 2),
        (8, 5),
        (5, 3),
        (16, 9),
        (2, 1),
        (21, 9),
    ];
    let landscape = width >= height;
    let ratio = f64::from(width.max(height)) / f64::from(width.min(height));
    let &(ratio_width, ratio_height) = COMMON_RATIOS
        .iter()
        .min_by(|left, right| {
            let left_distance = (ratio - f64::from(left.0) / f64::from(left.1)).abs();
            let right_distance = (ratio - f64::from(right.0) / f64::from(right.1)).abs();
            left_distance.total_cmp(&right_distance)
        })
        .expect("common aspect ratios must not be empty");
    if landscape {
        format!("{ratio_width}:{ratio_height}")
    } else {
        format!("{ratio_height}:{ratio_width}")
    }
}

fn fitted_thumbnail_size(dimensions: (u32, u32), maximum: f32) -> (f32, f32) {
    let (width, height) = dimensions;
    if width == 0 || height == 0 || maximum <= 0.0 {
        return (0.0, 0.0);
    }
    let scale = (maximum / width as f32).min(maximum / height as f32);
    (width as f32 * scale, height as f32 * scale)
}

fn build_preview(
    path: PathBuf,
    processing: ProcessingOptions,
    export: ExportOptions,
    softproof: SoftproofSettings,
    cancel: Arc<AtomicBool>,
) -> Result<PreviewBuildOutcome, String> {
    if preview_cancelled(&cancel) {
        return Ok(PreviewBuildOutcome::Cancelled);
    }
    let loaded = load_image(&path)
        .context("failed to load image")
        .map_err(|error| error.to_string())?;
    if preview_cancelled(&cancel) {
        return Ok(PreviewBuildOutcome::Cancelled);
    }
    let source_icc_profile = loaded.icc_profile;
    let image = downscale_for_preview(loaded.image);
    let preview_crop = crop_rect(image.width(), image.height(), processing.print_size)
        .context("failed to calculate preview crop")
        .map_err(|error| error.to_string())?;
    let crop_display = apply_source_profile_to_srgb(&image, source_icc_profile.as_deref())
        .context("failed to color-manage crop preview")
        .map_err(|error| error.to_string())?;
    let crop_proofed = if softproof.profile_path().is_some() {
        Some(
            apply_preview_profile_with_source(&image, &softproof, source_icc_profile.as_deref())
                .context("failed to apply crop softproof preview")
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    if preview_cancelled(&cancel) {
        return Ok(PreviewBuildOutcome::Cancelled);
    }
    let bordered =
        match add_preview_border_with_cancel(&image, &processing, || preview_cancelled(&cancel)) {
            Ok(image) => image,
            Err(ProcessingError::Cancelled) => return Ok(PreviewBuildOutcome::Cancelled),
            Err(error) => {
                return Err(anyhow::Error::new(error)
                    .context("failed to add border")
                    .to_string());
            }
        };
    if preview_cancelled(&cancel) {
        return Ok(PreviewBuildOutcome::Cancelled);
    }
    let compressed = compression_preview_image(bordered, &export)
        .context("failed to simulate compression preview")
        .map_err(|error| error.to_string())?;
    if preview_cancelled(&cancel) {
        return Ok(PreviewBuildOutcome::Cancelled);
    }
    let proofed = if softproof.profile_path().is_some() {
        Some(
            apply_preview_profile_with_source(
                &compressed,
                &softproof,
                source_icc_profile.as_deref(),
            )
            .context("failed to apply softproof preview")
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    let display = apply_source_profile_to_srgb(&compressed, source_icc_profile.as_deref())
        .context("failed to color-manage preview")
        .map_err(|error| error.to_string())?;
    if preview_cancelled(&cancel) {
        return Ok(PreviewBuildOutcome::Cancelled);
    }
    Ok(PreviewBuildOutcome::Ready(PreviewImages {
        base: PreviewBitmap::from_dynamic(&display),
        softproof: proofed.as_ref().map(PreviewBitmap::from_dynamic),
        crop_base: PreviewBitmap::from_dynamic(&crop_display),
        crop_softproof: crop_proofed.as_ref().map(PreviewBitmap::from_dynamic),
        crop_rect: preview_crop,
    }))
}

fn preview_cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Acquire)
}

fn preview_refresh_is_current(expected_request_id: u64, current_request_id: u64) -> bool {
    expected_request_id == current_request_id
}

fn preview_replacement_can_start(
    refresh_ready: bool,
    worker_active: bool,
    has_selected_file: bool,
) -> bool {
    refresh_ready && !worker_active && has_selected_file
}

fn export_worker_count(job_count: usize) -> usize {
    if job_count == 0 {
        return 0;
    }
    thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .saturating_sub(1)
        .clamp(1, 4)
        .min(job_count)
}

fn export_batch(
    jobs: Vec<(PathBuf, PathBuf)>,
    runtime: ExportRuntime,
    results: async_channel::Sender<BatchFileResult>,
) {
    let export_job = |(input, output): &(PathBuf, PathBuf)| {
        let result = export_one(
            input,
            output,
            runtime.processing,
            runtime.export_options,
            runtime.output_profile.as_deref(),
            &runtime.budget,
            &runtime.cancel,
        );
        if results.send_blocking(result).is_err() {
            runtime.cancel.store(true, Ordering::Release);
        }
    };
    match runtime.pool {
        Some(pool) => pool.install(|| jobs.par_iter().for_each(export_job)),
        None => jobs.iter().for_each(export_job),
    }
}

fn export_one(
    input: &Path,
    output: &Path,
    processing: ProcessingOptions,
    export: ExportOptions,
    output_profile: Option<&[u8]>,
    budget: &Arc<ProcessingBudget>,
    cancel: &AtomicBool,
) -> BatchFileResult {
    if cancel.load(Ordering::Acquire) {
        return cancelled_export_result(input, output);
    }
    let reservation_cancelled = AtomicBool::new(false);
    let input_reserved = AtomicU64::new(0);
    let loaded = load_image_with_reservations(
        input,
        || cancel.load(Ordering::Acquire),
        |bytes| {
            input_reserved.store(bytes, Ordering::Release);
            budget.acquire(bytes, Some(cancel)).map_err(|error| {
                if error == "export cancelled" {
                    reservation_cancelled.store(true, Ordering::Release);
                }
                error
            })
        },
        |metadata| {
            let requirements = processing_requirements(
                metadata.dimensions.0,
                metadata.dimensions.1,
                metadata.bit_depth,
                &processing,
            )
            .map_err(|error| error.to_string())?;
            let required_bytes = export_processing_bytes(
                &requirements,
                metadata.dimensions,
                metadata.bit_depth,
                metadata.format,
                output_profile.is_some(),
            )?;
            let input_bytes = input_reserved.load(Ordering::Acquire);
            if required_bytes
                .checked_add(input_bytes)
                .is_none_or(|combined| combined > MAX_CONCURRENT_PROCESSING_BYTES)
            {
                return Err(format!(
                    "decode and processing require more than {} bytes",
                    MAX_CONCURRENT_PROCESSING_BYTES
                ));
            }
            budget
                .acquire(required_bytes, Some(cancel))
                .map_err(|error| {
                    if error == "export cancelled" {
                        reservation_cancelled.store(true, Ordering::Release);
                    }
                    error
                })
        },
    );
    let (loaded, _permit) = match loaded {
        Ok(loaded) => loaded,
        Err(_)
            if reservation_cancelled.load(Ordering::Acquire) || cancel.load(Ordering::Acquire) =>
        {
            return cancelled_export_result(input, output);
        }
        Err(error) => {
            return failed_export_result(
                input,
                output,
                anyhow::Error::new(error).context("failed to load image"),
            );
        }
    };
    if cancel.load(Ordering::Acquire) {
        return cancelled_export_result(input, output);
    }
    let source_bit_depth = loaded.bit_depth;
    let source_icc_profile = loaded.icc_profile;
    let mut source_image = loaded.image;
    let converted_before_processing =
        output_profile.is_some() && source_bit_depth == SourceBitDepth::Other;
    if let Some(output_profile) = output_profile.filter(|_| converted_before_processing) {
        source_image = match convert_to_output_profile(
            source_image,
            source_icc_profile.as_deref(),
            output_profile,
            || cancel.load(Ordering::Acquire),
        ) {
            Ok(image) => image,
            Err(SoftproofError::Cancelled) => return cancelled_export_result(input, output),
            Err(error) => {
                return failed_export_result(
                    input,
                    output,
                    anyhow::Error::new(error).context("failed to convert output color profile"),
                );
            }
        };
    }
    let export = match output_ppi(source_image.width(), source_image.height(), &processing) {
        Ok(ppi) => export.with_pixel_density(ppi.x.round() as u32, ppi.y.round() as u32),
        Err(error) => {
            return failed_export_result(
                input,
                output,
                anyhow::Error::new(error).context("failed to calculate output resolution"),
            );
        }
    };
    let image = match add_border_with_cancel(&source_image, &processing, || {
        cancel.load(Ordering::Acquire)
    }) {
        Ok(image) => image,
        Err(ProcessingError::Cancelled) => return cancelled_export_result(input, output),
        Err(error) => {
            return failed_export_result(
                input,
                output,
                anyhow::Error::new(error).context("failed to add border"),
            );
        }
    };
    drop(source_image);
    if cancel.load(Ordering::Acquire) {
        return cancelled_export_result(input, output);
    }
    let (image, embedded_profile) = match output_profile.filter(|_| !converted_before_processing) {
        Some(output_profile) => match convert_to_output_profile(
            image,
            source_icc_profile.as_deref(),
            output_profile,
            || cancel.load(Ordering::Acquire),
        ) {
            Ok(image) => (image, Some(output_profile)),
            Err(SoftproofError::Cancelled) => return cancelled_export_result(input, output),
            Err(error) => {
                return failed_export_result(
                    input,
                    output,
                    anyhow::Error::new(error).context("failed to convert output color profile"),
                );
            }
        },
        None if converted_before_processing => (image, output_profile),
        None => (image, source_icc_profile.as_deref()),
    };
    match save_image_with_icc_profile_and_cancel(&image, output, &export, embedded_profile, || {
        cancel.load(Ordering::Acquire)
    }) {
        Ok(()) => BatchFileResult {
            input: input.to_path_buf(),
            planned_output: output.to_path_buf(),
            output: Some(output.to_path_buf()),
            error: None,
            cancelled: false,
        },
        Err(ExportError::Cancelled) => cancelled_export_result(input, output),
        Err(error) => failed_export_result(
            input,
            output,
            anyhow::Error::new(error).context("failed to save image"),
        ),
    }
}

fn failed_export_result(input: &Path, output: &Path, error: anyhow::Error) -> BatchFileResult {
    BatchFileResult {
        input: input.to_path_buf(),
        planned_output: output.to_path_buf(),
        output: None,
        error: Some(error.to_string()),
        cancelled: false,
    }
}

fn cancelled_export_result(input: &Path, output: &Path) -> BatchFileResult {
    BatchFileResult {
        input: input.to_path_buf(),
        planned_output: output.to_path_buf(),
        output: None,
        error: None,
        cancelled: true,
    }
}

fn export_completion_status(results: &[BatchFileResult]) -> StatusMessage {
    let cancelled = results.iter().filter(|result| result.cancelled).count();
    let failed = results
        .iter()
        .filter(|result| !result.cancelled && result.error.is_some())
        .count();
    let succeeded = results.len().saturating_sub(failed + cancelled);
    if cancelled > 0 {
        let message = rust_i18n::t!(
            "export-cancelled-summary",
            succeeded = succeeded,
            cancelled = cancelled,
            failed = failed
        )
        .into_owned();
        if failed == 0 {
            StatusMessage::ok(message)
        } else {
            StatusMessage::error(message)
        }
    } else if failed == 0 {
        StatusMessage::ok(i18n::plural("export-completed", succeeded))
    } else {
        StatusMessage::error(
            rust_i18n::t!(
                "export-completed-with-errors",
                succeeded = succeeded,
                failed = failed
            )
            .into_owned(),
        )
    }
}

fn export_jobs_preflight(
    jobs: Vec<(PathBuf, PathBuf)>,
    plan: &BatchExportPlan,
    cancel: &AtomicBool,
) -> ExportPreflight {
    let output_profile = match &plan.output_profile {
        Some(profile) => Ok(Some(profile.clone())),
        None => plan
            .output_profile_path
            .as_deref()
            .map(load_rgb_output_profile)
            .transpose()
            .map(|profile| profile.map(Arc::<[u8]>::from)),
    };
    if cancel.load(Ordering::Acquire) {
        let results = jobs
            .into_iter()
            .map(|(input, output)| cancelled_export_result(&input, &output))
            .collect();
        return ExportPreflight {
            jobs: Vec::new(),
            results,
            output_profile: None,
        };
    }
    let output_profile = match output_profile {
        Ok(profile) => profile,
        Err(error) => {
            let error = error.to_string();
            let results = jobs
                .into_iter()
                .map(|(input, output)| BatchFileResult {
                    input,
                    planned_output: output,
                    output: None,
                    error: Some(error.clone()),
                    cancelled: false,
                })
                .collect();
            return ExportPreflight {
                jobs: Vec::new(),
                results,
                output_profile: None,
            };
        }
    };
    let mut ready = Vec::with_capacity(jobs.len());
    let mut results = Vec::new();
    for (input, output) in jobs {
        if cancel.load(Ordering::Acquire) {
            results.push(cancelled_export_result(&input, &output));
            continue;
        }
        let metadata = load_image_metadata(&input);
        if cancel.load(Ordering::Acquire) {
            results.push(cancelled_export_result(&input, &output));
            continue;
        }
        let metadata = match metadata {
            Ok(metadata) => metadata,
            Err(error) => {
                results.push(failed_export_result(
                    &input,
                    &output,
                    anyhow::Error::new(error).context("failed to inspect image"),
                ));
                continue;
            }
        };
        if !can_export_bit_depth(metadata.bit_depth, &plan.export) {
            results.push(BatchFileResult {
                input,
                planned_output: output,
                output: None,
                error: Some(tr("export-preflight-depth")),
                cancelled: false,
            });
            continue;
        }
        let requirements = processing_requirements(
            metadata.dimensions.0,
            metadata.dimensions.1,
            metadata.bit_depth,
            &plan.processing,
        )
        .map_err(|error| error.to_string())
        .and_then(|requirements| {
            export_processing_bytes(
                &requirements,
                metadata.dimensions,
                metadata.bit_depth,
                metadata.format,
                output_profile.is_some(),
            )
            .map(|_| requirements)
        });
        if let Err(error) = requirements {
            let name = input
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image")
                .to_owned();
            results.push(BatchFileResult {
                input,
                planned_output: output,
                output: None,
                error: Some(
                    rust_i18n::t!("export-preflight-processing", name = name, error = error)
                        .into_owned(),
                ),
                cancelled: false,
            });
            continue;
        }
        ready.push((input, output));
    }
    ExportPreflight {
        jobs: ready,
        results,
        output_profile,
    }
}

fn export_processing_bytes(
    requirements: &crate::processing::ProcessingRequirements,
    source_dimensions: (u32, u32),
    source_bit_depth: SourceBitDepth,
    source_format: Option<image::ImageFormat>,
    convert_profile: bool,
) -> Result<u64, String> {
    let bytes_per_pixel = match source_bit_depth {
        SourceBitDepth::Eight => 14_u64,
        SourceBitDepth::Sixteen => 28_u64,
        SourceBitDepth::Other => 56_u64,
    };
    let conversion_pixels = if source_bit_depth == SourceBitDepth::Other {
        u64::from(source_dimensions.0).checked_mul(u64::from(source_dimensions.1))
    } else {
        u64::from(requirements.output_width).checked_mul(u64::from(requirements.output_height))
    }
    .ok_or_else(|| "color conversion dimensions exceed supported size".to_owned())?;
    let conversion_bytes = if convert_profile {
        conversion_pixels
            .checked_mul(bytes_per_pixel)
            .ok_or_else(|| "color conversion dimensions exceed supported size".to_owned())?
    } else {
        0
    };
    let source_bytes_per_pixel = match source_bit_depth {
        SourceBitDepth::Eight => 4_u64,
        SourceBitDepth::Sixteen => 8_u64,
        SourceBitDepth::Other => 16_u64,
    };
    let tiff_decode_bytes = if source_format == Some(image::ImageFormat::Tiff) {
        u64::from(source_dimensions.0)
            .checked_mul(u64::from(source_dimensions.1))
            .and_then(|pixels| pixels.checked_mul(source_bytes_per_pixel))
            .and_then(|bytes| bytes.checked_mul(4))
            .ok_or_else(|| "TIFF decode dimensions exceed supported size".to_owned())?
    } else {
        0
    };
    let required = requirements
        .estimated_bytes
        .max(conversion_bytes)
        .max(tiff_decode_bytes);
    if required > MAX_CONCURRENT_PROCESSING_BYTES {
        return Err(format!(
            "color conversion requires approximately {} MiB; limit is {} MiB",
            required.div_ceil(1024 * 1024),
            MAX_CONCURRENT_PROCESSING_BYTES / (1024 * 1024)
        ));
    }
    Ok(required)
}

fn retryable_jobs(results: &[BatchFileResult]) -> Vec<(PathBuf, PathBuf)> {
    results
        .iter()
        .filter(|result| result.cancelled || result.error.is_some())
        .map(|result| (result.input.clone(), result.planned_output.clone()))
        .collect()
}

fn successful_results(results: &[BatchFileResult]) -> Vec<BatchFileResult> {
    results
        .iter()
        .filter(|result| result.output.is_some() && result.error.is_none() && !result.cancelled)
        .cloned()
        .collect()
}

fn open_output_folder(path: &Path) -> io::Result<()> {
    if !path.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "output folder does not exist",
        ));
    }

    #[cfg(target_os = "windows")]
    let status = Command::new("explorer").arg(path).status()?;
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(path).status()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open").arg(path).status()?;

    #[cfg(any(target_os = "windows", target_os = "macos", unix))]
    return if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "folder opener exited with status {status}"
        )))
    };

    #[allow(unreachable_code)]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "opening folders is unsupported on this platform",
    ))
}

fn planned_jobs(
    files: &[PathBuf],
    output_dir: &Path,
    format: OutputFormat,
) -> Vec<(PathBuf, PathBuf)> {
    let mut reserved = HashSet::new();
    files
        .iter()
        .map(|input| {
            let mut output = unique_output_path(input, output_dir, format);
            if reserved.contains(&output) {
                let stem = input
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .filter(|stem| !stem.is_empty())
                    .unwrap_or("image");
                let extension = crate::export::extension(format);
                for index in 1.. {
                    let candidate = output_dir.join(format!("{stem}_preprint_{index}.{extension}"));
                    if !candidate.exists() && !reserved.contains(&candidate) {
                        output = candidate;
                        break;
                    }
                }
            }
            reserved.insert(output.clone());
            (input.clone(), output)
        })
        .collect()
}

fn downscale_for_preview(image: DynamicImage) -> DynamicImage {
    let longest = image.width().max(image.height());
    if longest <= MAX_PREVIEW_DIM {
        return image;
    }
    image.thumbnail(MAX_PREVIEW_DIM, MAX_PREVIEW_DIM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidate_preview_clears_stale_result_state() {
        crate::i18n::init();
        let mut app = PreprintApp {
            preview_request_id: 7,
            ..PreprintApp::default()
        };
        app.preview.rendering = true;
        app.preview.set_compression_label("JPEG q80");
        app.preview_image_size = Some([100, 200]);
        app.invalidate_preview();
        assert_eq!(app.preview_request_id, 8);
        assert!(!app.preview.rendering);
        assert!(app.preview_base.is_none());
        assert!(app.preview_image_size.is_none());
    }

    #[test]
    fn invalidating_preview_cancels_active_render() {
        crate::i18n::init();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut app = PreprintApp {
            preview_cancel: Some(cancel.clone()),
            ..PreprintApp::default()
        };

        app.invalidate_preview();

        assert!(cancel.load(Ordering::Acquire));
        assert!(app.preview_cancel.is_none());
    }

    #[test]
    fn cancelled_preview_stops_before_loading_image() {
        let cancel = Arc::new(AtomicBool::new(true));

        let result = build_preview(
            PathBuf::from("missing.png"),
            ProcessingOptions::new(PrintSizeMm::new(600.0, 400.0), 8.0, BorderStyle::White),
            ExportOptions::new(OutputFormat::Png, 90),
            SoftproofSettings::default(),
            cancel,
        )
        .unwrap();

        assert!(matches!(result, PreviewBuildOutcome::Cancelled));
    }

    #[test]
    fn debounced_preview_refresh_requires_current_idle_request() {
        assert!(preview_refresh_is_current(4, 4));
        assert!(!preview_refresh_is_current(4, 5));
        assert!(preview_replacement_can_start(true, false, true));
        assert!(!preview_replacement_can_start(true, true, true));
        assert!(!preview_replacement_can_start(true, false, false));
    }

    #[test]
    fn print_readiness_flags_low_resolution_and_accepts_print_quality() {
        crate::i18n::init();
        let entry_with_dimensions = |dimensions| FileEntry {
            path: PathBuf::from("photo.tif"),
            status: Some(FileStatus {
                dimensions: Some(dimensions),
                bit_depth: Some(SourceBitDepth::Sixteen),
                format: Some(ImageFormat::Tiff),
                color_space: Some("sRGB".to_owned()),
                thumbnail: None,
                error: None,
            }),
        };

        let low =
            entry_with_dimensions((1200, 800)).print_readiness(PrintSizeMm::new(600.0, 400.0));
        assert!(low.warning);
        assert!(!low.ready);

        let excellent =
            entry_with_dimensions((7200, 4800)).print_readiness(PrintSizeMm::new(600.0, 400.0));
        assert!(!excellent.warning);
        assert!(excellent.ready);
    }

    #[test]
    fn export_worker_count_is_bounded() {
        assert_eq!(export_worker_count(0), 0);
        assert_eq!(export_worker_count(1), 1);
        assert!(export_worker_count(20) <= 4);
    }

    #[test]
    fn processing_budget_releases_capacity_with_permit() {
        let budget = Arc::new(ProcessingBudget::new(100));
        let first = budget.acquire(60, None).unwrap();
        let second = budget.acquire(40, None).unwrap();
        assert_eq!(budget.in_use(), 100);

        drop(first);
        assert_eq!(budget.in_use(), 40);
        drop(second);
        assert_eq!(budget.in_use(), 0);
    }

    #[test]
    fn processing_budget_wakes_waiter_after_release() {
        use std::{sync::mpsc, time::Duration};

        let budget = Arc::new(ProcessingBudget::new(100));
        let permit = budget.acquire(100, None).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let waiting_budget = budget.clone();
        let waiter = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _permit = waiting_budget.acquire(1, None).unwrap();
            acquired_tx.send(()).unwrap();
        });

        started_rx.recv().unwrap();
        assert!(acquired_rx.recv_timeout(Duration::from_millis(20)).is_err());
        drop(permit);
        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        waiter.join().unwrap();
    }

    #[test]
    fn processing_budget_wait_stops_after_cancellation() {
        use std::{sync::mpsc, time::Duration};

        let budget = Arc::new(ProcessingBudget::new(100));
        let permit = budget.acquire(100, None).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let waiting_budget = budget.clone();
        let waiting_cancel = cancel.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let waiter = thread::spawn(move || {
            result_tx
                .send(waiting_budget.acquire(1, Some(&waiting_cancel)))
                .unwrap();
        });

        cancel.store(true, Ordering::Release);
        let result = result_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(result.err().as_deref(), Some("export cancelled"));
        drop(permit);
        waiter.join().unwrap();
    }

    #[test]
    fn export_releases_runtime_budget_after_save() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.png");
        let output = dir.path().join("output.png");
        DynamicImage::new_rgba8(2, 2).save(&input).unwrap();
        let budget = Arc::new(ProcessingBudget::new(MAX_CONCURRENT_PROCESSING_BYTES));
        let cancel = AtomicBool::new(false);

        let result = export_one(
            &input,
            &output,
            ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 0.0, BorderStyle::White),
            ExportOptions::new(OutputFormat::Png, 90),
            None,
            &budget,
            &cancel,
        );

        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(budget.in_use(), 0);
        assert!(output.exists());
    }

    #[test]
    fn export_converts_and_embeds_selected_rgb_profile() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.png");
        let output = dir.path().join("output.png");
        DynamicImage::new_rgba8(2, 2).save(&input).unwrap();
        let profile = lcms2::Profile::new_srgb();
        profile.set_encoded_icc_version(0x0210_0000);
        let profile = profile.icc().unwrap();
        let budget = Arc::new(ProcessingBudget::new(MAX_CONCURRENT_PROCESSING_BYTES));
        let cancel = AtomicBool::new(false);

        let result = export_one(
            &input,
            &output,
            ProcessingOptions::new(PrintSizeMm::new(25.4, 25.4), 0.0, BorderStyle::White),
            ExportOptions::new(OutputFormat::Png, 90),
            Some(&profile),
            &budget,
            &cancel,
        );

        assert!(result.error.is_none(), "{:?}", result.error);
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(output).unwrap(),
        ));
        let embedded = decoder
            .read_info()
            .unwrap()
            .info()
            .icc_profile
            .as_ref()
            .unwrap()
            .to_vec();
        assert_eq!(embedded, profile);
        assert_eq!(budget.in_use(), 0);
    }

    #[test]
    fn color_conversion_memory_is_included_in_runtime_reservation() {
        let requirements = crate::processing::ProcessingRequirements {
            output_width: 100,
            output_height: 50,
            estimated_bytes: 1,
        };

        assert_eq!(
            export_processing_bytes(&requirements, (100, 50), SourceBitDepth::Eight, None, true,)
                .unwrap(),
            70_000
        );
        assert_eq!(
            export_processing_bytes(
                &requirements,
                (100, 50),
                SourceBitDepth::Sixteen,
                None,
                true,
            )
            .unwrap(),
            140_000
        );
        assert_eq!(
            export_processing_bytes(&requirements, (200, 50), SourceBitDepth::Other, None, true,)
                .unwrap(),
            560_000
        );
        assert_eq!(
            export_processing_bytes(
                &requirements,
                (100, 50),
                SourceBitDepth::Eight,
                Some(image::ImageFormat::Tiff),
                false,
            )
            .unwrap(),
            80_000
        );
    }

    #[test]
    fn export_batch_publishes_each_file_result() {
        let dir = tempfile::tempdir().unwrap();
        let jobs: Vec<_> = (0..2)
            .map(|index| {
                let input = dir.path().join(format!("input-{index}.png"));
                let output = dir.path().join(format!("output-{index}.png"));
                DynamicImage::new_rgba8(2, 2).save(&input).unwrap();
                (input, output)
            })
            .collect();
        let budget = Arc::new(ProcessingBudget::new(MAX_CONCURRENT_PROCESSING_BYTES));
        let cancel = Arc::new(AtomicBool::new(false));
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .unwrap(),
        );
        let (sender, receiver) = async_channel::unbounded();

        export_batch(
            jobs,
            ExportRuntime {
                processing: ProcessingOptions::new(
                    PrintSizeMm::new(25.4, 25.4),
                    0.0,
                    BorderStyle::White,
                ),
                export_options: ExportOptions::new(OutputFormat::Png, 90),
                output_profile: None,
                pool: Some(pool),
                budget: budget.clone(),
                cancel,
            },
            sender,
        );

        let results = [
            receiver.recv_blocking().unwrap(),
            receiver.recv_blocking().unwrap(),
        ];
        assert!(receiver.recv_blocking().is_err());
        assert!(results.iter().all(|result| result.error.is_none()));
        assert!(results.iter().all(|result| result.output.is_some()));
        assert_eq!(budget.in_use(), 0);
    }

    #[test]
    fn export_completion_reports_success_and_failures() {
        crate::i18n::init();
        let successful = BatchFileResult {
            input: PathBuf::from("input.png"),
            planned_output: PathBuf::from("output.png"),
            output: Some(PathBuf::from("output.png")),
            error: None,
            cancelled: false,
        };
        let failed = BatchFileResult {
            input: PathBuf::from("broken.png"),
            planned_output: PathBuf::from("broken-out.png"),
            output: None,
            error: Some("broken".into()),
            cancelled: false,
        };

        let status = export_completion_status(&[successful]);
        assert_eq!(status.text, "Export completed: 1 image.");
        assert!(!status.is_error);

        let status = export_completion_status(&[failed]);
        assert_eq!(status.text, "Export completed: 0 succeeded, 1 failed.");
        assert!(status.is_error);

        let status = export_completion_status(&[cancelled_export_result(
            Path::new("later.png"),
            Path::new("later-out.png"),
        )]);
        assert_eq!(
            status.text,
            "Export cancelled: 0 succeeded, 1 cancelled, 0 failed."
        );
        assert!(!status.is_error);
    }

    #[test]
    fn retry_selects_only_failed_and_cancelled_inputs() {
        let results = vec![
            BatchFileResult {
                input: PathBuf::from("complete.png"),
                planned_output: PathBuf::from("complete-out.png"),
                output: Some(PathBuf::from("complete-out.png")),
                error: None,
                cancelled: false,
            },
            BatchFileResult {
                input: PathBuf::from("failed.png"),
                planned_output: PathBuf::from("failed-out.png"),
                output: None,
                error: Some("failed".into()),
                cancelled: false,
            },
            cancelled_export_result(Path::new("cancelled.png"), Path::new("cancelled-out.png")),
        ];

        assert_eq!(
            retryable_jobs(&results),
            [
                (PathBuf::from("failed.png"), PathBuf::from("failed-out.png")),
                (
                    PathBuf::from("cancelled.png"),
                    PathBuf::from("cancelled-out.png")
                )
            ]
        );
    }

    #[test]
    fn retry_retains_prior_successful_results() {
        let successful = BatchFileResult {
            input: PathBuf::from("complete.png"),
            planned_output: PathBuf::from("complete-out.png"),
            output: Some(PathBuf::from("complete-out.png")),
            error: None,
            cancelled: false,
        };
        let failed = BatchFileResult {
            input: PathBuf::from("failed.png"),
            planned_output: PathBuf::from("failed-out.png"),
            output: None,
            error: Some("failed".into()),
            cancelled: false,
        };

        let retained = successful_results(&[successful.clone(), failed]);

        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].input, successful.input);
        assert_eq!(retained[0].output, successful.output);
    }

    #[test]
    fn planned_jobs_number_repeated_stems_from_original_name() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            PathBuf::from("first/photo.png"),
            PathBuf::from("second/photo.png"),
            PathBuf::from("third/photo.png"),
        ];

        let jobs = planned_jobs(&files, dir.path(), OutputFormat::Png);
        let outputs: Vec<_> = jobs.into_iter().map(|(_, output)| output).collect();

        assert_eq!(
            outputs,
            [
                dir.path().join("photo_preprint.png"),
                dir.path().join("photo_preprint_1.png"),
                dir.path().join("photo_preprint_2.png"),
            ]
        );
    }

    #[test]
    fn reveal_rejects_missing_output_folder_without_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");

        let error = open_output_folder(&missing).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn discovers_supported_images_in_nested_folders() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let first = dir.path().join("first.png");
        let second = nested.join("second.jpg");
        DynamicImage::new_rgba8(1, 1).save(&first).unwrap();
        DynamicImage::new_rgb8(1, 1).save(&second).unwrap();
        std::fs::write(nested.join("notes.txt"), b"not an image").unwrap();

        let discovery = discover_import_paths(vec![dir.path().to_path_buf()]);

        assert_eq!(discovery.images, vec![first, second]);
        assert_eq!(discovery.skipped, 1);
    }

    #[cfg(unix)]
    #[test]
    fn follows_symlinked_folders_without_recursing_cycles() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let linked = dir.path().join("linked");
        std::fs::create_dir(&source).unwrap();
        let image = source.join("image.png");
        DynamicImage::new_rgba8(1, 1).save(&image).unwrap();
        symlink(&source, &linked).unwrap();
        symlink(dir.path(), source.join("cycle")).unwrap();

        let discovery = discover_import_paths(vec![linked.clone()]);

        assert_eq!(discovery.images, vec![linked.join("image.png")]);
        assert_eq!(discovery.skipped, 0);
    }

    #[test]
    fn deduplicates_existing_and_repeated_import_paths() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.png");
        let second = dir.path().join("second.png");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();

        let (unique, duplicates) = deduplicate_import_paths(
            [first.as_path()].into_iter(),
            vec![first.clone(), second.clone(), second.clone()],
        );

        assert_eq!(unique, vec![second]);
        assert_eq!(duplicates, 2);
    }

    #[test]
    fn import_summary_reports_added_duplicate_and_skipped_counts() {
        crate::i18n::init();

        assert_eq!(
            import_summary(2, 1, 3, 0),
            "Added 2 images. 1 duplicate skipped. 3 unsupported or unreadable files skipped."
        );
    }

    #[test]
    fn prepares_only_new_readable_images() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("existing.png");
        let added = dir.path().join("added.png");
        let broken = dir.path().join("broken.png");
        DynamicImage::new_rgba8(1, 1).save(&existing).unwrap();
        DynamicImage::new_rgba8(1, 1).save(&added).unwrap();
        std::fs::write(&broken, b"not an image").unwrap();

        let prepared = prepare_import(
            vec![existing.clone(), added.clone(), broken],
            vec![existing],
        );

        assert_eq!(prepared.entries.len(), 1);
        assert_eq!(prepared.entries[0].path, added);
        assert_eq!(prepared.duplicates, 1);
        assert_eq!(prepared.skipped, 1);
        assert_eq!(prepared.limited, 0);
    }

    #[test]
    fn import_stops_at_queue_limit() {
        let dir = tempfile::tempdir().unwrap();
        let candidate = dir.path().join("candidate.png");
        DynamicImage::new_rgba8(1, 1).save(&candidate).unwrap();
        let existing = (0..MAX_QUEUE_FILES)
            .map(|index| dir.path().join(format!("existing-{index}.png")))
            .collect();

        let prepared = prepare_import(vec![candidate], existing);

        assert!(prepared.entries.is_empty());
        assert_eq!(prepared.limited, 1);
    }

    #[test]
    fn length_units_round_trip_centimeters() {
        for unit in LengthUnit::ALL {
            let displayed = unit.display_value(60.0);
            assert!((unit.to_centimeters(displayed) - 60.0).abs() < 0.001);
        }
        assert_eq!(format_length(23.622047), "23.622");
    }

    #[test]
    fn print_presets_match_both_orientations() {
        assert_eq!(PrintPreset::matching(29.7, 21.0), PrintPreset::A4);
        assert_eq!(PrintPreset::matching(21.0, 29.7), PrintPreset::A4);
        assert_eq!(PrintPreset::matching(31.0, 20.0), PrintPreset::Custom);
    }

    #[test]
    fn workflow_preferences_apply_and_snapshot_without_loss() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = WorkflowPreferences {
            print_width_cm: 42.0,
            print_height_cm: 29.7,
            border_mm: 4.0,
            length_unit: LengthUnit::Inches,
            border_style: "black".into(),
            output_format: "tiff".into(),
            quality: 84,
            bit_depth: 16,
            png_compression: 8,
            tiff_compression: "lzw".into(),
            tiff_deflate_level: "fast".into(),
            softproof_profile: Some(dir.path().join("printer.icc")),
            convert_output_profile: true,
            output_dir: Some(dir.path().to_path_buf()),
        };
        let mut app = PreprintApp::default();

        app.apply_workflow_preferences(&workflow);

        assert_eq!(app.print_preset, PrintPreset::A3);
        assert_eq!(app.workflow_preferences(), workflow);
    }

    #[test]
    fn decimal_input_accepts_editing_and_locale_separator() {
        assert!(decimal_input_is_valid("", 200.0));
        assert!(decimal_input_is_valid(".", 200.0));
        assert!(decimal_input_is_valid(",", 200.0));
        assert!(decimal_input_is_valid("10.5", 200.0));
        assert!(decimal_input_is_valid("10,5", 200.0));
        assert_eq!(parse_decimal("10,5"), Some(10.5));
        assert!(!decimal_input_is_valid("10,5.2", 200.0));
        assert!(!decimal_input_is_valid("201", 200.0));
    }

    #[test]
    fn integer_input_accepts_replacement_but_enforces_maximum() {
        assert!(integer_input_is_valid("", 100));
        assert!(integer_input_is_valid("90", 100));
        assert!(!integer_input_is_valid("101", 100));
        assert!(!integer_input_is_valid("9.5", 100));
    }

    #[test]
    fn preview_zoom_input_and_wheel_direction_are_bounded() {
        assert!(preview_zoom_input_is_valid(""));
        assert!(preview_zoom_input_is_valid("9"));
        assert!(preview_zoom_input_is_valid("10"));
        assert!(preview_zoom_input_is_valid("800"));
        assert!(!preview_zoom_input_is_valid("801"));
        assert_eq!(
            scroll_zoom_direction(ScrollDelta::Lines(gpui::point(0.0, 1.0))),
            1
        );
        assert_eq!(
            scroll_zoom_direction(ScrollDelta::Lines(gpui::point(0.0, -1.0))),
            -1
        );
        assert_eq!(preview_zoom_key_bindings().len(), 3);
    }

    #[test]
    fn file_inspection_builds_bounded_thumbnail_and_complete_metadata() {
        crate::i18n::init();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.png");
        DynamicImage::new_rgb8(300, 200).save(&path).unwrap();

        let status = inspect_file(&path);
        let entry = FileEntry {
            path,
            status: Some(status.clone()),
        };
        let thumbnail = status.thumbnail.unwrap();
        let decoded = image::load_from_memory(&thumbnail.bytes).unwrap();
        let (primary, secondary) = entry.metadata_lines();

        assert!(decoded.width() <= THUMBNAIL_MAX_DIMENSION);
        assert!(decoded.height() <= THUMBNAIL_MAX_DIMENSION);
        assert_eq!(status.format, Some(ImageFormat::Png));
        assert_eq!(status.dimensions, Some((300, 200)));
        assert!(primary.contains("PNG · 3:2 · 300 × 200 px · 0.1 MP"));
        assert!(secondary.contains("sRGB (assumed) · 8-bit"));
    }

    #[test]
    fn aspect_ratio_uses_nearest_standard_label() {
        assert_eq!(aspect_ratio_label(6000, 4000), "3:2");
        assert_eq!(aspect_ratio_label(4000, 6000), "2:3");
        assert_eq!(aspect_ratio_label(6048, 4024), "3:2");
        assert_eq!(aspect_ratio_label(4032, 3024), "4:3");
        assert_eq!(aspect_ratio_label(3024, 4032), "3:4");
    }

    #[test]
    fn thumbnail_fit_preserves_aspect_ratio_inside_square() {
        assert_eq!(fitted_thumbnail_size((400, 200), 88.0), (88.0, 44.0));
        assert_eq!(fitted_thumbnail_size((200, 400), 88.0), (44.0, 88.0));
        assert_eq!(fitted_thumbnail_size((300, 300), 88.0), (88.0, 88.0));
    }

    #[test]
    fn sixteen_bit_default_falls_back_for_eight_bit_source() {
        let mut app = PreprintApp::default();
        app.files.push(FileEntry {
            path: PathBuf::from("eight-bit.tiff"),
            status: Some(FileStatus {
                dimensions: Some((100, 100)),
                bit_depth: Some(SourceBitDepth::Eight),
                format: Some(ImageFormat::Tiff),
                color_space: Some("sRGB".into()),
                thumbnail: None,
                error: None,
            }),
        });

        app.normalize_bit_depth_choice();

        assert_eq!(app.bit_depth, BitDepth::Eight);
    }

    #[test]
    fn sixteen_bit_default_falls_back_for_mixed_batch() {
        let mut app = PreprintApp::default();
        for (name, bit_depth) in [
            ("sixteen-bit.tiff", SourceBitDepth::Sixteen),
            ("eight-bit.tiff", SourceBitDepth::Eight),
        ] {
            app.files.push(FileEntry {
                path: PathBuf::from(name),
                status: Some(FileStatus {
                    dimensions: Some((100, 100)),
                    bit_depth: Some(bit_depth),
                    format: Some(ImageFormat::Tiff),
                    color_space: Some("sRGB".into()),
                    thumbnail: None,
                    error: None,
                }),
            });
        }

        app.normalize_bit_depth_choice();

        assert_eq!(app.bit_depth, BitDepth::Eight);
    }

    #[test]
    fn export_preflight_returns_unreadable_files_as_failures() {
        crate::i18n::init();
        let dir = tempfile::tempdir().unwrap();
        let jobs = vec![(dir.path().join("broken.tiff"), dir.path().join("out.tiff"))];
        let plan = BatchExportPlan {
            output_dir: dir.path().to_path_buf(),
            processing: ProcessingOptions::new(
                PrintSizeMm::new(600.0, 400.0),
                8.0,
                BorderStyle::White,
            ),
            export: ExportOptions::new(OutputFormat::Tiff, 90),
            output_profile_path: None,
            output_profile: None,
        };
        let cancel = AtomicBool::new(false);

        let preflight = export_jobs_preflight(jobs, &plan, &cancel);

        assert!(preflight.jobs.is_empty());
        assert_eq!(preflight.results.len(), 1);
        assert!(
            preflight.results[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("failed to inspect image"))
        );
    }

    #[test]
    fn export_preflight_returns_wrong_depth_as_per_file_failure() {
        crate::i18n::init();
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("eight-bit.png");
        DynamicImage::new_rgba8(2, 2).save(&input).unwrap();
        let mut options = ExportOptions::new(OutputFormat::Tiff, 90);
        options.bit_depth = BitDepth::Sixteen;
        let jobs = vec![(input, dir.path().join("out.tiff"))];
        let plan = BatchExportPlan {
            output_dir: dir.path().to_path_buf(),
            processing: ProcessingOptions::new(
                PrintSizeMm::new(600.0, 400.0),
                8.0,
                BorderStyle::White,
            ),
            export: options,
            output_profile_path: None,
            output_profile: None,
        };
        let cancel = AtomicBool::new(false);

        let preflight = export_jobs_preflight(jobs, &plan, &cancel);

        assert!(preflight.jobs.is_empty());
        assert_eq!(preflight.results.len(), 1);
        assert_eq!(
            preflight.results[0].error.as_deref(),
            Some("16-bit export requires every image in the batch to be 16-bit.")
        );
    }

    #[test]
    fn export_preflight_keeps_valid_jobs_when_another_file_fails() {
        crate::i18n::init();
        let dir = tempfile::tempdir().unwrap();
        let valid = dir.path().join("valid.png");
        let broken = dir.path().join("broken.png");
        DynamicImage::new_rgba8(2, 2).save(&valid).unwrap();
        std::fs::write(&broken, b"broken").unwrap();
        let valid_output = dir.path().join("valid-out.png");
        let broken_output = dir.path().join("broken-out.png");
        let jobs = vec![
            (valid.clone(), valid_output.clone()),
            (broken.clone(), broken_output.clone()),
        ];
        let plan = BatchExportPlan {
            output_dir: dir.path().to_path_buf(),
            processing: ProcessingOptions::new(
                PrintSizeMm::new(25.4, 25.4),
                0.0,
                BorderStyle::White,
            ),
            export: ExportOptions::new(OutputFormat::Png, 90),
            output_profile_path: None,
            output_profile: None,
        };
        let cancel = AtomicBool::new(false);

        let preflight = export_jobs_preflight(jobs, &plan, &cancel);

        assert_eq!(preflight.jobs, [(valid, valid_output)]);
        assert_eq!(preflight.results.len(), 1);
        assert_eq!(preflight.results[0].input, broken);
        assert_eq!(preflight.results[0].planned_output, broken_output);
        assert!(preflight.results[0].error.is_some());
    }

    #[test]
    fn export_preflight_loads_selected_rgb_output_profile_once() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.png");
        let profile_path = dir.path().join("output.icc");
        DynamicImage::new_rgba8(2, 2).save(&input).unwrap();
        let profile = lcms2::Profile::new_srgb().icc().unwrap();
        std::fs::write(&profile_path, &profile).unwrap();
        let jobs = vec![(input.clone(), dir.path().join("output.png"))];
        let plan = BatchExportPlan {
            output_dir: dir.path().to_path_buf(),
            processing: ProcessingOptions::new(
                PrintSizeMm::new(25.4, 25.4),
                0.0,
                BorderStyle::White,
            ),
            export: ExportOptions::new(OutputFormat::Png, 90),
            output_profile_path: Some(profile_path),
            output_profile: None,
        };
        let cancel = AtomicBool::new(false);

        let preflight = export_jobs_preflight(jobs, &plan, &cancel);

        assert_eq!(preflight.jobs.len(), 1);
        assert!(preflight.results.is_empty());
        assert_eq!(
            preflight.output_profile.as_deref(),
            Some(profile.as_slice())
        );

        let mut retry_plan = plan;
        retry_plan.output_profile = preflight.output_profile;
        std::fs::write(
            retry_plan.output_profile_path.as_ref().unwrap(),
            lcms2::Profile::new_xyz().icc().unwrap(),
        )
        .unwrap();
        let retry = export_jobs_preflight(
            vec![(input, dir.path().join("retry.png"))],
            &retry_plan,
            &cancel,
        );
        assert_eq!(retry.jobs.len(), 1);
        assert!(retry.results.is_empty());
        assert_eq!(retry.output_profile.as_deref(), Some(profile.as_slice()));
    }

    #[test]
    fn export_preflight_rejects_non_rgb_output_profile_for_all_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let profile_path = dir.path().join("xyz.icc");
        std::fs::write(&profile_path, lcms2::Profile::new_xyz().icc().unwrap()).unwrap();
        let jobs = vec![(PathBuf::from("input.png"), PathBuf::from("output.png"))];
        let plan = BatchExportPlan {
            output_dir: dir.path().to_path_buf(),
            processing: ProcessingOptions::new(
                PrintSizeMm::new(25.4, 25.4),
                0.0,
                BorderStyle::White,
            ),
            export: ExportOptions::new(OutputFormat::Png, 90),
            output_profile_path: Some(profile_path),
            output_profile: None,
        };
        let cancel = AtomicBool::new(false);

        let preflight = export_jobs_preflight(jobs, &plan, &cancel);

        assert!(preflight.jobs.is_empty());
        assert_eq!(preflight.results.len(), 1);
        assert!(
            preflight.results[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("must use RGB color space"))
        );
        assert!(preflight.output_profile.is_none());
    }

    #[test]
    fn export_preflight_marks_jobs_cancelled_without_inspection() {
        let jobs = vec![
            (PathBuf::from("first.png"), PathBuf::from("first-out.png")),
            (PathBuf::from("second.png"), PathBuf::from("second-out.png")),
        ];
        let plan = BatchExportPlan {
            output_dir: PathBuf::from("output"),
            processing: ProcessingOptions::new(
                PrintSizeMm::new(25.4, 25.4),
                0.0,
                BorderStyle::White,
            ),
            export: ExportOptions::new(OutputFormat::Png, 90),
            output_profile_path: None,
            output_profile: None,
        };
        let cancel = AtomicBool::new(true);

        let preflight = export_jobs_preflight(jobs, &plan, &cancel);

        assert!(preflight.jobs.is_empty());
        assert_eq!(preflight.results.len(), 2);
        assert!(preflight.results.iter().all(|result| result.cancelled));
    }

    #[test]
    fn sixteen_bit_default_is_retained_until_source_is_known() {
        let mut app = PreprintApp::default();

        app.normalize_bit_depth_choice();

        assert_eq!(app.bit_depth, BitDepth::Sixteen);
    }
}
