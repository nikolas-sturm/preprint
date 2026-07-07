use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
};

use anyhow::{Context, Result};
use eframe::egui;
use egui_i18n::tr;
use egui_phosphor_icons::icons;
use image::DynamicImage;
use image::GenericImageView;
use rayon::prelude::*;
use rfd::FileDialog;

use crate::{
    export::{
        BitDepth, ExportOptions, OutputFormat, TiffCompression, TiffDeflateLevel,
        can_export_bit_depth, compression_preview_image, compression_preview_label, save_image,
        unique_output_path,
    },
    i18n,
    loader::{SourceBitDepth, load_image, load_image_metadata},
    processing::{
        BorderStyle, PrintSizeMm, ProcessingOptions, add_border, aspect_ratio_warning,
        calculate_ppi,
    },
    softproof::{SoftproofSettings, apply_preview_profile},
};

pub struct PreprintApp {
    files: Vec<FileEntry>,
    selected_index: usize,
    print_width_cm: f32,
    print_height_cm: f32,
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
    preview: PreviewState,
    preview_base_texture: Option<egui::TextureHandle>,
    preview_base_nearest_texture: Option<egui::TextureHandle>,
    preview_softproof_texture: Option<egui::TextureHandle>,
    preview_softproof_nearest_texture: Option<egui::TextureHandle>,
    preview_image_size: Option<[usize; 2]>,
    preview_receiver: Option<Receiver<PreviewMessage>>,
    preview_request_id: u64,
    status_message: Option<StatusMessage>,
    batch: Option<BatchState>,
}

impl Default for PreprintApp {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            selected_index: 0,
            print_width_cm: 10.0,
            print_height_cm: 15.0,
            border_mm: 5.0,
            border_style: BorderStyle::White,
            output_format: OutputFormat::Png,
            quality: 90,
            bit_depth: BitDepth::Eight,
            png_compression: 6,
            tiff_compression: TiffCompression::Deflate,
            tiff_deflate_level: TiffDeflateLevel::Balanced,
            output_dir: None,
            softproof: SoftproofSettings::default(),
            preview: PreviewState::default(),
            preview_base_texture: None,
            preview_base_nearest_texture: None,
            preview_softproof_texture: None,
            preview_softproof_nearest_texture: None,
            preview_image_size: None,
            preview_receiver: None,
            preview_request_id: 0,
            status_message: None,
            batch: None,
        }
    }
}

const MIN_MAGNIFIER_ZOOM: f32 = 2.0;
const MAX_MAGNIFIER_ZOOM: f32 = 12.0;
const MIN_MAGNIFIER_RADIUS: f32 = 60.0;
const MAX_MAGNIFIER_RADIUS: f32 = 220.0;

#[derive(Clone, Copy)]
struct ThemePalette {
    accent: egui::Color32,
    accent_hover: egui::Color32,
    accent_dim: egui::Color32,
    text_primary: egui::Color32,
    text_secondary: egui::Color32,
    dim_text: egui::Color32,
    faint_text: egui::Color32,
    card_bg: egui::Color32,
    card_stroke: egui::Color32,
    hairline: egui::Color32,
    error_color: egui::Color32,
    ok_color: egui::Color32,
    warn_color: egui::Color32,
    panel_fill: egui::Color32,
    window_fill: egui::Color32,
    extreme_bg: egui::Color32,
    faint_bg: egui::Color32,
    inactive_bg: egui::Color32,
    hovered_weak_bg: egui::Color32,
    open_weak_bg: egui::Color32,
}

impl ThemePalette {
    fn dark() -> Self {
        Self {
            accent: egui::Color32::from_rgb(96, 165, 250),
            accent_hover: egui::Color32::from_rgb(124, 184, 255),
            accent_dim: egui::Color32::from_rgb(58, 96, 140),
            text_primary: egui::Color32::from_rgb(242, 245, 250),
            text_secondary: egui::Color32::from_rgb(202, 208, 220),
            dim_text: egui::Color32::from_rgb(172, 179, 194),
            faint_text: egui::Color32::from_rgb(144, 151, 168),
            card_bg: egui::Color32::from_rgb(28, 31, 36),
            card_stroke: egui::Color32::from_rgb(46, 50, 58),
            hairline: egui::Color32::from_rgb(40, 44, 52),
            error_color: egui::Color32::from_rgb(245, 122, 122),
            ok_color: egui::Color32::from_rgb(120, 200, 140),
            warn_color: egui::Color32::from_rgb(240, 190, 92),
            panel_fill: egui::Color32::from_rgb(22, 24, 28),
            window_fill: egui::Color32::from_rgb(28, 31, 36),
            extreme_bg: egui::Color32::from_rgb(16, 18, 22),
            faint_bg: egui::Color32::from_rgb(26, 28, 33),
            inactive_bg: egui::Color32::from_rgb(34, 37, 43),
            hovered_weak_bg: egui::Color32::from_rgb(42, 46, 55),
            open_weak_bg: egui::Color32::from_rgb(38, 42, 50),
        }
    }

    fn light() -> Self {
        Self {
            accent: egui::Color32::from_rgb(37, 99, 235),
            accent_hover: egui::Color32::from_rgb(29, 78, 216),
            accent_dim: egui::Color32::from_rgb(147, 197, 253),
            text_primary: egui::Color32::from_rgb(24, 28, 36),
            text_secondary: egui::Color32::from_rgb(58, 66, 82),
            dim_text: egui::Color32::from_rgb(95, 104, 122),
            faint_text: egui::Color32::from_rgb(122, 131, 148),
            card_bg: egui::Color32::from_rgb(255, 255, 255),
            card_stroke: egui::Color32::from_rgb(218, 222, 230),
            hairline: egui::Color32::from_rgb(224, 228, 236),
            error_color: egui::Color32::from_rgb(200, 50, 50),
            ok_color: egui::Color32::from_rgb(40, 150, 80),
            warn_color: egui::Color32::from_rgb(180, 120, 20),
            panel_fill: egui::Color32::from_rgb(242, 244, 247),
            window_fill: egui::Color32::from_rgb(255, 255, 255),
            extreme_bg: egui::Color32::from_rgb(228, 232, 238),
            faint_bg: egui::Color32::from_rgb(236, 239, 243),
            inactive_bg: egui::Color32::from_rgb(236, 239, 243),
            hovered_weak_bg: egui::Color32::from_rgb(226, 230, 236),
            open_weak_bg: egui::Color32::from_rgb(230, 234, 240),
        }
    }

    fn for_theme(theme: egui::Theme) -> Self {
        match theme {
            egui::Theme::Dark => Self::dark(),
            egui::Theme::Light => Self::light(),
        }
    }
}

fn palette_id() -> egui::Id {
    egui::Id::new("preprint-theme-palette")
}

fn store_palette(ctx: &egui::Context, palette: ThemePalette) {
    ctx.data_mut(|d| d.insert_temp(palette_id(), palette));
}

fn palette(ctx: &egui::Context) -> ThemePalette {
    ctx.data(|d| d.get_temp::<ThemePalette>(palette_id()))
        .unwrap_or_else(ThemePalette::dark)
}

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "dat"];

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn ppi_quality(min_ppi: f32, p: &ThemePalette) -> (egui::Color32, String) {
    if min_ppi < 150.0 {
        (p.error_color, tr!("ppi-quality-low"))
    } else if min_ppi < 300.0 {
        (p.warn_color, tr!("ppi-quality-ok"))
    } else {
        (p.ok_color, tr!("ppi-quality-sharp"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewState {
    pub open: bool,
    pub fullscreen: bool,
    pub fit_to_window: bool,
    pub rendering: bool,
    progress: f32,
    progress_label: String,
    softproof_enabled: bool,
    compression_label: String,
    magnifier_enabled: bool,
    magnifier_zoom: f32,
    magnifier_radius: f32,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            open: false,
            fullscreen: false,
            fit_to_window: true,
            rendering: false,
            progress: 0.0,
            progress_label: tr!("idle"),
            softproof_enabled: true,
            compression_label: String::new(),
            magnifier_enabled: false,
            magnifier_zoom: 4.0,
            magnifier_radius: 120.0,
        }
    }
}

impl PreviewState {
    pub fn mark_rendering(&mut self) {
        self.open = true;
        self.rendering = true;
        self.set_progress(0.05, tr!("starting-preview"));
    }

    fn mark_finished(&mut self) {
        self.rendering = false;
        self.set_progress(1.0, tr!("preview-ready"));
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

    pub fn magnifier_enabled(&self) -> bool {
        self.magnifier_enabled
    }

    pub fn set_magnifier_enabled(&mut self, enabled: bool) {
        self.magnifier_enabled = enabled;
    }

    pub fn magnifier_zoom(&self) -> f32 {
        self.magnifier_zoom
    }

    pub fn set_magnifier_zoom(&mut self, zoom: f32) {
        self.magnifier_zoom = zoom.clamp(MIN_MAGNIFIER_ZOOM, MAX_MAGNIFIER_ZOOM);
    }

    pub fn magnifier_radius(&self) -> f32 {
        self.magnifier_radius
    }

    pub fn set_magnifier_radius(&mut self, radius: f32) {
        self.magnifier_radius = radius.clamp(MIN_MAGNIFIER_RADIUS, MAX_MAGNIFIER_RADIUS);
    }
}

impl PreprintApp {
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

    pub fn sixteen_bit_tiff_available(source: SourceBitDepth, format: OutputFormat) -> bool {
        format == OutputFormat::Tiff && source == SourceBitDepth::Sixteen
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
        }
    }

    fn selected_entry(&self) -> Option<&FileEntry> {
        self.files.get(self.selected_index)
    }

    fn selected_file(&self) -> Option<&Path> {
        self.selected_entry().map(|entry| entry.path.as_path())
    }

    fn selected_source_bit_depth(&self) -> Option<SourceBitDepth> {
        self.selected_entry()
            .and_then(|entry| entry.status.as_ref())
            .and_then(|status| status.bit_depth)
    }

    fn clear_preview_result(&mut self) {
        self.preview_base_texture = None;
        self.preview_base_nearest_texture = None;
        self.preview_softproof_texture = None;
        self.preview_softproof_nearest_texture = None;
        self.preview_image_size = None;
        self.preview.set_compression_label("");
    }

    fn invalidate_preview(&mut self) {
        self.preview_request_id = self.preview_request_id.wrapping_add(1);
        self.preview_receiver = None;
        self.preview.rendering = false;
        self.preview.set_progress(0.0, tr!("idle"));
        self.clear_preview_result();
        self.preview.open = false;
    }

    fn poll_batch(&mut self) {
        if let Some(batch) = &mut self.batch {
            while let Ok(message) = batch.receiver.try_recv() {
                match message {
                    BatchMessage::File(result) => {
                        batch.completed += 1;
                        batch.results.push(result);
                    }
                    BatchMessage::Finished => {
                        batch.running = false;
                    }
                }
            }
        }
    }

    fn poll_preview(&mut self, ctx: &egui::Context) {
        let mut messages = Vec::new();
        if let Some(receiver) = &self.preview_receiver {
            while let Ok(message) = receiver.try_recv() {
                messages.push(message);
            }
        }

        if messages.is_empty() {
            return;
        }

        for message in messages {
            match message {
                PreviewMessage::Progress {
                    request_id,
                    progress,
                    label,
                } if request_id == self.preview_request_id => {
                    self.preview.set_progress(progress, label.clone());
                    self.status_message = Some(StatusMessage::ok(label));
                }
                PreviewMessage::Finished { request_id, result }
                    if request_id == self.preview_request_id =>
                {
                    self.preview_receiver = None;
                    self.preview.mark_finished();

                    match result {
                        Ok(images) => {
                            let base = dynamic_to_color_image(&images.base);
                            let size = base.size;
                            let base_nearest = base.clone();
                            self.preview_base_texture = Some(ctx.load_texture(
                                "base-preview",
                                base,
                                egui::TextureOptions::LINEAR,
                            ));
                            self.preview_base_nearest_texture = Some(ctx.load_texture(
                                "base-preview-nearest",
                                base_nearest,
                                egui::TextureOptions::NEAREST,
                            ));
                            self.preview_softproof_texture =
                                images.softproof.as_ref().map(|image| {
                                    ctx.load_texture(
                                        "softproof-preview",
                                        dynamic_to_color_image(&image),
                                        egui::TextureOptions::LINEAR,
                                    )
                                });
                            self.preview_softproof_nearest_texture =
                                images.softproof.map(|image| {
                                    ctx.load_texture(
                                        "softproof-preview-nearest",
                                        dynamic_to_color_image(&image),
                                        egui::TextureOptions::NEAREST,
                                    )
                                });
                            self.preview_image_size = Some(size);
                            self.preview.set_compression_label(images.compression_label);
                            self.status_message = Some(StatusMessage::ok(tr!("preview-ready")));
                        }
                        Err(error) => {
                            self.clear_preview_result();
                            self.preview.open = false;
                            self.status_message = Some(StatusMessage::error(error));
                        }
                    }
                }
                _ => {}
            }
        }

        ctx.request_repaint();
    }

    fn pick_files(&mut self) {
        if let Some(files) = FileDialog::new()
            .add_filter("Images", IMAGE_EXTENSIONS)
            .pick_files()
        {
            self.set_files(files);
        }
    }

    fn set_files(&mut self, files: Vec<PathBuf>) {
        let kept: Vec<PathBuf> = files.into_iter().filter(|p| is_image_path(p)).collect();
        if kept.is_empty() {
            return;
        }
        self.files = kept
            .into_iter()
            .map(|path| FileEntry {
                status: Some(inspect_file(&path)),
                path,
            })
            .collect();
        self.selected_index = 0;
        self.invalidate_preview();
        let count = self.files.len();
        self.status_message = Some(StatusMessage::ok(
            tr!("loaded-images", { count: count as i32 }),
        ));
        self.normalize_bit_depth_choice();
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        self.set_files(dropped);
    }

    fn files_hovered(ctx: &egui::Context) -> bool {
        ctx.input(|i| !i.raw.hovered_files.is_empty())
    }

    fn pick_output_folder(&mut self) {
        if let Some(folder) = FileDialog::new().pick_folder() {
            self.output_dir = Some(folder);
        }
    }

    fn pick_icc_profile(&mut self) {
        if let Some(profile) = FileDialog::new()
            .add_filter("ICC profiles", &["icc", "icm"])
            .pick_file()
        {
            self.softproof.set_profile(profile);
        }
    }

    fn update_preview(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected_file().map(Path::to_path_buf) else {
            self.status_message = Some(StatusMessage::error(tr!("select-image-to-preview")));
            return;
        };

        self.preview_request_id = self.preview_request_id.wrapping_add(1);
        let request_id = self.preview_request_id;
        let processing = self.processing_options();
        let export = self.export_options();
        let mut softproof = self.softproof.clone();
        softproof.set_enabled(true);
        let (sender, receiver) = mpsc::channel();
        let worker_ctx = ctx.clone();

        self.preview_receiver = Some(receiver);
        self.preview.mark_rendering();
        self.status_message = Some(StatusMessage::ok(tr!("rendering")));

        thread::spawn(move || {
            send_preview_progress(&sender, request_id, 0.10, tr!("loading-image"));
            let result = load_image(&path)
                .context("failed to load image")
                .map(|loaded| downscale_for_preview(loaded.image))
                .and_then(|image| {
                    send_preview_progress(&sender, request_id, 0.35, tr!("applying-border"));
                    add_border(&image, &processing).context("failed to add border")
                })
                .and_then(|base| {
                    send_preview_progress(&sender, request_id, 0.58, tr!("simulating-compression"));
                    let display = compression_preview_image(base, &export)
                        .context("failed to simulate compression preview")?;
                    let compression_label = compression_preview_label(&export);

                    if softproof.profile_path().is_some() {
                        send_preview_progress(&sender, request_id, 0.75, tr!("applying-icc"));
                        let proofed = apply_preview_profile(&display, &softproof)
                            .context("failed to apply softproof preview")?;
                        Ok(PreviewImages {
                            base: display,
                            softproof: Some(proofed),
                            compression_label,
                        })
                    } else {
                        send_preview_progress(&sender, request_id, 0.85, tr!("preparing-texture"));
                        Ok(PreviewImages {
                            base: display,
                            softproof: None,
                            compression_label,
                        })
                    }
                })
                .map_err(|error| error.to_string());

            let _ = sender.send(PreviewMessage::Finished { request_id, result });
            worker_ctx.request_repaint();
        });

        ctx.request_repaint();
    }

    fn start_export(&mut self) {
        if self.batch.as_ref().is_some_and(|batch| batch.running) {
            return;
        }

        let Some(output_dir) = self.output_dir.clone() else {
            self.status_message = Some(StatusMessage::error(tr!("pick-output-folder")));
            return;
        };

        if self.files.is_empty() {
            self.status_message = Some(StatusMessage::error(tr!("add-images-to-export")));
            return;
        }

        let files: Vec<PathBuf> = self.files.iter().map(|entry| entry.path.clone()).collect();
        let jobs = planned_jobs(&files, &output_dir, self.output_format);
        let total = jobs.len();
        let processing = self.processing_options();
        let export = self.export_options();
        let worker_count = export_worker_count(total);
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            if worker_count <= 1 {
                for (input, output) in &jobs {
                    let result = export_one(input, output, processing, export);
                    let _ = sender.send(BatchMessage::File(result));
                }
            } else {
                match rayon::ThreadPoolBuilder::new()
                    .num_threads(worker_count)
                    .build()
                {
                    Ok(pool) => pool.install(|| {
                        jobs.par_iter()
                            .for_each_with(sender.clone(), |sender, (input, output)| {
                                let result = export_one(input, output, processing, export);
                                let _ = sender.send(BatchMessage::File(result));
                            });
                    }),
                    Err(_) => {
                        for (input, output) in &jobs {
                            let result = export_one(input, output, processing, export);
                            let _ = sender.send(BatchMessage::File(result));
                        }
                    }
                }
            }
            let _ = sender.send(BatchMessage::Finished);
        });

        self.batch = Some(BatchState {
            total,
            completed: 0,
            running: true,
            receiver,
            results: Vec::new(),
        });
        self.status_message = Some(StatusMessage::ok(
            tr!("export-started", { count: worker_count as i32 }),
        ));
    }

    fn normalize_bit_depth_choice(&mut self) {
        let selected_depth = self.selected_source_bit_depth();
        let options = self.export_options();
        let allowed = selected_depth
            .map(|depth| can_export_bit_depth(depth, &options))
            .unwrap_or(false);

        if self.bit_depth == BitDepth::Sixteen && !allowed {
            self.bit_depth = BitDepth::Eight;
        }
    }

    fn configure_style(ctx: &egui::Context) {
        let p = ThemePalette::for_theme(ctx.theme());
        store_palette(ctx, p);

        ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(10.0, 8.0);
            style.spacing.button_padding = egui::vec2(12.0, 7.0);
            style.spacing.window_margin = egui::Margin::same(14);
            style.spacing.indent = 14.0;
            style.spacing.combo_width = 180.0;
            style
                .text_styles
                .insert(egui::TextStyle::Small, egui::FontId::proportional(11.0));
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
            style
                .text_styles
                .insert(egui::TextStyle::Monospace, egui::FontId::monospace(12.0));
            style.visuals.panel_fill = p.panel_fill;
            style.visuals.window_fill = p.window_fill;
            style.visuals.extreme_bg_color = p.extreme_bg;
            style.visuals.faint_bg_color = p.faint_bg;
            style.visuals.selection.bg_fill = p.accent;
            style.visuals.selection.stroke = egui::Stroke::new(1.0, p.accent.gamma_multiply(1.4));
            style.visuals.widgets.noninteractive.fg_stroke =
                egui::Stroke::new(1.0, p.text_secondary);
            style.visuals.widgets.noninteractive.bg_fill = egui::Color32::TRANSPARENT;
            style.visuals.widgets.noninteractive.weak_bg_fill = egui::Color32::TRANSPARENT;
            style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.hairline);
            style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, p.text_secondary);
            style.visuals.widgets.inactive.bg_fill = p.inactive_bg;
            style.visuals.widgets.inactive.weak_bg_fill = p.inactive_bg;
            style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(7);
            style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, p.text_primary);
            style.visuals.widgets.hovered.weak_bg_fill = p.hovered_weak_bg;
            style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(7);
            style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, p.text_primary);
            style.visuals.widgets.active.weak_bg_fill = p.accent;
            style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(7);
            style.visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0, p.text_primary);
            style.visuals.widgets.open.weak_bg_fill = p.open_weak_bg;
            style.visuals.window_stroke = egui::Stroke::new(1.0, p.card_stroke);
        });
    }

    fn draw_top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(
                icons::IMAGE_SQUARE
                    .regular()
                    .color(palette(ui.ctx()).accent)
                    .size(22.0),
            );
            ui.add_space(2.0);
            ui.heading(
                egui::RichText::new("Preprint")
                    .strong()
                    .size(22.0)
                    .color(palette(ui.ctx()).text_primary),
            );
            ui.label(
                egui::RichText::new(tr!("tagline"))
                    .size(13.0)
                    .color(palette(ui.ctx()).dim_text),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let export_running = self.batch.as_ref().is_some_and(|batch| batch.running);
                let export_ready =
                    !self.files.is_empty() && self.output_dir.is_some() && !export_running;
                let export_label = format!(
                    "{}  {}",
                    icons::EXPORT.as_str(),
                    if export_running {
                        tr!("exporting")
                    } else {
                        tr!("export-all")
                    }
                );
                if primary_button(ui, &export_label, export_ready).clicked() {
                    self.start_export();
                }
                let preview_ready = self.selected_file().is_some() && !self.preview.rendering;
                let preview_label = format!(
                    "{}  {}",
                    icons::EYE.as_str(),
                    if self.preview.rendering {
                        tr!("rendering")
                    } else {
                        tr!("preview")
                    }
                );
                if ui
                    .add_enabled(preview_ready, egui::Button::new(preview_label))
                    .clicked()
                {
                    self.update_preview(ctx);
                }

                let current = i18n::current_language();
                let lang_label = format!(
                    "{}  {}",
                    icons::TRANSLATE.as_str(),
                    i18n::language_label(&current)
                );
                ui.menu_button(lang_label, |ui| {
                    for (id, label) in i18n::LANGUAGES {
                        if ui.selectable_label(current == *id, *label).clicked() {
                            i18n::set_language(id);
                            ctx.request_repaint();
                        }
                    }
                });

                let theme = ctx.theme();
                let (theme_icon, theme_hint) = match theme {
                    egui::Theme::Dark => (icons::SUN.as_str(), tr!("switch-to-light")),
                    egui::Theme::Light => (icons::MOON.as_str(), tr!("switch-to-dark")),
                };
                if ui
                    .add(egui::Button::new(theme_icon).frame(false))
                    .on_hover_text(theme_hint)
                    .clicked()
                {
                    ctx.set_theme(match theme {
                        egui::Theme::Dark => egui::Theme::Light,
                        egui::Theme::Light => egui::Theme::Dark,
                    });
                    ctx.request_repaint();
                }
            });
        });
    }

    fn draw_status_strip(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(
                egui::RichText::new(tr!(
                    "files-loaded",
                    { count: self.files.len() as i32 }
                ))
                .small()
                .color(palette(ui.ctx()).dim_text),
            );
            if let Some(message) = &self.status_message {
                dot_separator(ui);
                let (icon, color) = if message.is_error {
                    (icons::WARNING.as_str(), palette(ui.ctx()).error_color)
                } else {
                    (icons::CHECK_CIRCLE.as_str(), palette(ui.ctx()).ok_color)
                };
                ui.label(
                    egui::RichText::new(format!("{icon}  {}", message.text))
                        .small()
                        .color(color),
                );
            }
        });
    }

    fn draw_file_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let hovered = Self::files_hovered(ctx);
        let mut want_pick = false;
        let title = tr!("card-input-files");
        let subtitle = tr!("hint-drag-drop");
        let add_label = tr!("add-images");
        card_titled(
            ui,
            &title,
            Some(&subtitle),
            Some(|ui: &mut egui::Ui| {
                if ui
                    .button(format!("{}  {}", icons::PLUS.as_str(), add_label))
                    .clicked()
                {
                    want_pick = true;
                }
            }),
            |ui| {
                if self.files.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        let icon = if hovered {
                            icons::DOWNLOAD_SIMPLE.regular()
                        } else {
                            icons::IMAGES.regular()
                        };
                        ui.label(icon.size(30.0).color(if hovered {
                            palette(ui.ctx()).accent
                        } else {
                            palette(ui.ctx()).faint_text
                        }));
                        ui.add_space(8.0);
                        let headline = if hovered {
                            tr!("drop-to-load")
                        } else {
                            tr!("no-images")
                        };
                        ui.label(
                            egui::RichText::new(headline)
                                .size(15.0)
                                .color(if hovered {
                                    palette(ui.ctx()).accent
                                } else {
                                    palette(ui.ctx()).text_secondary
                                })
                                .strong(),
                        );
                        ui.add_space(4.0);
                        let hint = if hovered {
                            tr!("release-to-import")
                        } else {
                            tr!("add-or-drag")
                        };
                        ui.label(
                            egui::RichText::new(hint)
                                .small()
                                .color(palette(ui.ctx()).dim_text),
                        );
                        ui.add_space(20.0);
                    });
                    return;
                }

                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut next_selected = None;
                        let fallback_name = tr!("image-fallback-name");
                        for (index, entry) in self.files.iter().enumerate() {
                            let selected = self.selected_index == index;
                            let name = entry
                                .path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or(&fallback_name);
                            let status = entry.status_label();
                            let label = format!("{name}\n{status}");
                            let rich = if selected {
                                egui::RichText::new(label)
                                    .strong()
                                    .color(palette(ui.ctx()).text_primary)
                            } else {
                                egui::RichText::new(label).color(palette(ui.ctx()).text_secondary)
                            };
                            let response = ui
                                .selectable_label(selected, rich)
                                .on_hover_text(format!("{}\n{}", entry.path.display(), status));
                            ui.add_space(2.0);
                            if response.clicked() {
                                next_selected = Some(index);
                            }
                        }

                        if let Some(index) = next_selected {
                            self.selected_index = index;
                            self.invalidate_preview();
                            self.normalize_bit_depth_choice();
                        }
                    });
            },
        );
        if want_pick {
            self.pick_files();
        }
    }

    fn draw_print_card(&mut self, ui: &mut egui::Ui) {
        let title = tr!("card-print-setup");
        card(ui, &title, |ui| {
            let target_label = tr!("target-size");
            let border_width_label = tr!("border-width");
            let border_style_label = tr!("border-style");
            egui::Grid::new("print-setup-grid")
                .num_columns(2)
                .spacing([14.0, 10.0])
                .show(ui, |ui| {
                    ui.label(&target_label);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.print_width_cm)
                                .range(0.1..=200.0)
                                .speed(0.2)
                                .suffix(" cm"),
                        );
                        ui.label(egui::RichText::new("\u{00D7}").color(palette(ui.ctx()).dim_text));
                        ui.add(
                            egui::DragValue::new(&mut self.print_height_cm)
                                .range(0.1..=200.0)
                                .speed(0.2)
                                .suffix(" cm"),
                        );
                    });
                    ui.end_row();

                    ui.label(&border_width_label);
                    ui.add(
                        egui::DragValue::new(&mut self.border_mm)
                            .range(0.0..=200.0)
                            .speed(0.2)
                            .suffix(" mm"),
                    );
                    ui.end_row();

                    ui.label(&border_style_label);
                    egui::ComboBox::from_id_salt("border-style")
                        .selected_text(self.border_style.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.border_style,
                                BorderStyle::White,
                                BorderStyle::White.label(),
                            );
                            ui.selectable_value(
                                &mut self.border_style,
                                BorderStyle::Black,
                                BorderStyle::Black.label(),
                            );
                            ui.selectable_value(
                                &mut self.border_style,
                                BorderStyle::MirroredBlur,
                                BorderStyle::MirroredBlur.label(),
                            );
                        });
                    ui.end_row();
                });

            ui.add_space(8.0);
            self.draw_ppi(ui);
        });
    }

    fn draw_output_card(&mut self, ui: &mut egui::Ui) {
        let mut want_output = false;
        let title = tr!("card-output");
        card(ui, &title, |ui| {
            let save_to_label = tr!("save-to");
            ui.horizontal(|ui| {
                ui.label(&save_to_label);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let button_label = if self.output_dir.is_some() {
                        format!("{}  {}", icons::FOLDER_OPEN.as_str(), tr!("change"))
                    } else {
                        format!("{}  {}", icons::FOLDER_OPEN.as_str(), tr!("choose"))
                    };
                    if ui.button(button_label).clicked() {
                        want_output = true;
                    }
                    let (path_text, color) = match self.output_dir.as_deref() {
                        Some(path) => (format!("{}", path.display()), palette(ui.ctx()).dim_text),
                        None => (tr!("no-folder"), palette(ui.ctx()).faint_text),
                    };
                    let label_resp = ui.add(
                        egui::Label::new(egui::RichText::new(path_text).small().color(color))
                            .truncate(),
                    );
                    if let Some(path) = self.output_dir.as_deref() {
                        label_resp.on_hover_text(format!("{}", path.display()));
                    }
                });
            });
            ui.add_space(6.0);
            ui.add(egui::Separator::default().spacing(4.0));
            ui.add_space(6.0);

            let format_label = tr!("format");
            let quality_label = tr!("quality");
            let bit_depth_label = tr!("bit-depth");
            let effort_label = tr!("effort");
            let compression_label = tr!("compression");
            egui::Grid::new("output-grid")
                .num_columns(2)
                .spacing([14.0, 10.0])
                .show(ui, |ui| {
                    ui.label(&format_label);
                    egui::ComboBox::from_id_salt("output-format")
                        .selected_text(self.output_format.label())
                        .show_ui(ui, |ui| {
                            for format in OutputFormat::ALL {
                                ui.selectable_value(
                                    &mut self.output_format,
                                    format,
                                    format.label(),
                                );
                            }
                        });
                    ui.end_row();

                    if self.output_format == OutputFormat::Jpeg {
                        self.bit_depth = BitDepth::Eight;
                        ui.label(&quality_label);
                        ui.add(
                            egui::Slider::new(&mut self.quality, 1..=100)
                                .text("q")
                                .smallest_positive(1.0),
                        );
                        ui.end_row();
                    } else {
                        ui.label(&bit_depth_label);
                        self.draw_bit_depth_picker(ui);
                        ui.end_row();

                        match self.output_format {
                            OutputFormat::Png => {
                                ui.label(&effort_label);
                                ui.add(
                                    egui::Slider::new(&mut self.png_compression, 1..=9)
                                        .text("level")
                                        .smallest_positive(1.0),
                                );
                                ui.end_row();
                            }
                            OutputFormat::Tiff => {
                                ui.label(&compression_label);
                                egui::ComboBox::from_id_salt("tiff-compression")
                                    .selected_text(self.tiff_compression.label())
                                    .show_ui(ui, |ui| {
                                        for method in TiffCompression::ALL {
                                            ui.selectable_value(
                                                &mut self.tiff_compression,
                                                method,
                                                method.label(),
                                            );
                                        }
                                    });
                                ui.end_row();

                                if self.tiff_compression == TiffCompression::Deflate {
                                    ui.label(&effort_label);
                                    egui::ComboBox::from_id_salt("tiff-deflate-level")
                                        .selected_text(self.tiff_deflate_level.label())
                                        .show_ui(ui, |ui| {
                                            for level in TiffDeflateLevel::ALL {
                                                ui.selectable_value(
                                                    &mut self.tiff_deflate_level,
                                                    level,
                                                    level.label(),
                                                );
                                            }
                                        });
                                    ui.end_row();
                                }
                            }
                            OutputFormat::Jpeg => {}
                        }
                    }
                });

            ui.add_space(8.0);
            self.draw_bit_depth_note(ui);
        });
        if want_output {
            self.pick_output_folder();
        }
    }

    fn draw_bit_depth_picker(&mut self, ui: &mut egui::Ui) {
        let can_16 = self
            .selected_source_bit_depth()
            .is_some_and(|depth| Self::sixteen_bit_tiff_available(depth, self.output_format));

        if self.bit_depth == BitDepth::Sixteen && !can_16 {
            self.bit_depth = BitDepth::Eight;
        }

        egui::ComboBox::from_id_salt("bit-depth")
            .selected_text(self.bit_depth.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.bit_depth,
                    BitDepth::Eight,
                    BitDepth::Eight.label(),
                );
                ui.add_enabled_ui(can_16, |ui| {
                    ui.selectable_value(
                        &mut self.bit_depth,
                        BitDepth::Sixteen,
                        BitDepth::Sixteen.label(),
                    );
                });
            });
    }

    fn draw_bit_depth_note(&self, ui: &mut egui::Ui) {
        let key = match (self.output_format, self.selected_source_bit_depth()) {
            (OutputFormat::Tiff, Some(SourceBitDepth::Sixteen)) => "note-16-tiff-available",
            (OutputFormat::Tiff, Some(SourceBitDepth::Eight)) => "note-16-tiff-8-source",
            (OutputFormat::Tiff, _) => "note-16-tiff-needs-source",
            _ => "note-16-tiff-only",
        };
        ui.label(
            egui::RichText::new(tr!(key))
                .small()
                .color(palette(ui.ctx()).dim_text),
        );
    }

    fn draw_ppi(&self, ui: &mut egui::Ui) {
        let Some(entry) = self.selected_entry() else {
            ui.label(
                egui::RichText::new(tr!("ppi-select-image"))
                    .small()
                    .color(palette(ui.ctx()).dim_text),
            );
            return;
        };

        let Some(status) = &entry.status else {
            ui.label(
                egui::RichText::new(tr!("ppi-inspecting"))
                    .small()
                    .color(palette(ui.ctx()).dim_text),
            );
            return;
        };

        if let Some(error) = &status.error {
            ui.label(
                egui::RichText::new(error)
                    .small()
                    .color(palette(ui.ctx()).error_color),
            );
            return;
        }

        let Some((width, height)) = status.dimensions else {
            ui.label(
                egui::RichText::new(tr!("ppi-dimensions-unavailable"))
                    .small()
                    .color(palette(ui.ctx()).dim_text),
            );
            return;
        };

        match calculate_ppi(width, height, self.print_size()) {
            Ok(ppi) => {
                let min_ppi = ppi.x.min(ppi.y);
                let (color, quality) = ppi_quality(min_ppi, &palette(ui.ctx()));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(icons::CIRCLE.regular().color(color).size(11.0));
                    ui.label(
                        egui::RichText::new(format!("PPI  {:.0} \u{00D7} {:.0}", ppi.x, ppi.y))
                            .strong()
                            .color(palette(ui.ctx()).text_primary),
                    );
                    ui.label(egui::RichText::new(quality).small().color(color));
                });
                if aspect_ratio_warning(width, height, self.print_size()) {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(tr!("ppi-aspect-warning"))
                            .small()
                            .color(palette(ui.ctx()).warn_color),
                    );
                }
            }
            Err(error) => {
                ui.label(
                    egui::RichText::new(error.to_string())
                        .small()
                        .color(palette(ui.ctx()).error_color),
                );
            }
        }
    }

    fn draw_batch_card(&self, ui: &mut egui::Ui) {
        let title = tr!("card-export-status");
        card(ui, &title, |ui| {
            let Some(batch) = &self.batch else {
                ui.label(
                    egui::RichText::new(tr!("no-export-running"))
                        .small()
                        .color(palette(ui.ctx()).dim_text),
                );
                return;
            };

            let fraction = if batch.total == 0 {
                0.0
            } else {
                batch.completed as f32 / batch.total as f32
            };
            let done = batch.completed == batch.total && !batch.running;
            let text = format!(
                "{} / {}{}",
                batch.completed,
                batch.total,
                if done {
                    format!("  {}", icons::CHECK.as_str())
                } else {
                    String::new()
                }
            );
            let bar = egui::ProgressBar::new(fraction).text(text);
            let bar = if done {
                bar.fill(palette(ui.ctx()).ok_color)
            } else {
                bar.fill(palette(ui.ctx()).accent)
            };
            ui.add(bar);

            if !batch.results.is_empty() {
                ui.add_space(4.0);
            }
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .show(ui, |ui| {
                    for result in &batch.results {
                        match &result.error {
                            Some(error) => {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} {} {}",
                                        icons::WARNING.as_str(),
                                        result.input.display(),
                                        error
                                    ))
                                    .small()
                                    .color(palette(ui.ctx()).error_color),
                                );
                            }
                            None => {
                                if let Some(output) = &result.output {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} saved {}",
                                            icons::CHECK.as_str(),
                                            output.display()
                                        ))
                                        .small()
                                        .color(palette(ui.ctx()).ok_color),
                                    );
                                }
                            }
                        }
                    }
                });
        });
    }

    fn draw_preview_summary(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut want_icc = false;
        let mut want_clear_icc = false;
        let title = tr!("card-preview");
        card(ui, &title, |ui| {
            let softproof_label = tr!("softproof-profile");
            ui.horizontal(|ui| {
                ui.label(&softproof_label);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.softproof.profile_path().is_some()
                        && ui
                            .button(format!("{}  {}", icons::X.as_str(), tr!("clear")))
                            .clicked()
                    {
                        want_clear_icc = true;
                    }
                    let profile_action = if self.softproof.profile_path().is_some() {
                        tr!("change")
                    } else {
                        tr!("choose")
                    };
                    if ui
                        .button(format!("{}  {}", icons::PALETTE.as_str(), profile_action,))
                        .clicked()
                    {
                        want_icc = true;
                    }
                    let profile_fallback = tr!("profile-fallback");
                    let (profile_text, color) = match self.softproof.profile_path() {
                        Some(path) => {
                            let name = path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(&profile_fallback);
                            (name.to_owned(), palette(ui.ctx()).dim_text)
                        }
                        None => (tr!("no-softproof"), palette(ui.ctx()).faint_text),
                    };
                    let label_resp = ui.add(
                        egui::Label::new(egui::RichText::new(profile_text).small().color(color))
                            .truncate(),
                    );
                    if let Some(path) = self.softproof.profile_path() {
                        label_resp.on_hover_text(format!("{}", path.display()));
                    }
                });
            });
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(tr!("preview-window-hint"))
                    .small()
                    .color(palette(ui.ctx()).dim_text),
            );
            if self.softproof.profile_path().is_some() {
                ui.label(
                    egui::RichText::new(tr!("profile-change-hint"))
                        .small()
                        .color(palette(ui.ctx()).faint_text),
                );
            }
            ui.add_space(6.0);
            if self.preview.rendering {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new(tr!("rendering-background"))
                            .small()
                            .color(palette(ui.ctx()).dim_text),
                    );
                });
            }
            if let Some(size) = self.preview_image_size {
                ui.label(
                    egui::RichText::new(tr!(
                        "preview-size",
                        { width: size[0] as i32, height: size[1] as i32 }
                    ))
                    .small()
                    .color(palette(ui.ctx()).dim_text),
                );
            }
            if !self.preview.compression_label().is_empty() {
                ui.label(
                    egui::RichText::new(self.preview.compression_label())
                        .small()
                        .color(palette(ui.ctx()).dim_text),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let can_refresh = self.selected_file().is_some() && !self.preview.rendering;
                let refresh_label = format!(
                    "{}  {}",
                    icons::ARROW_CLOCKWISE.as_str(),
                    if self.preview.rendering {
                        tr!("rendering")
                    } else {
                        tr!("refresh-preview")
                    }
                );
                if ui
                    .add_enabled(can_refresh, egui::Button::new(refresh_label))
                    .clicked()
                {
                    self.update_preview(ctx);
                }
                let show_label = format!("{}  {}", icons::ARROWS_OUT.as_str(), tr!("show-window"));
                if ui
                    .add_enabled(
                        self.preview_base_texture.is_some(),
                        egui::Button::new(show_label),
                    )
                    .clicked()
                {
                    self.preview.open = true;
                }
            });
        });
        if want_icc {
            self.pick_icc_profile();
        }
        if want_clear_icc {
            self.softproof.clear_profile();
            self.invalidate_preview();
            self.status_message = Some(StatusMessage::ok(tr!("cleared-softproof")));
        }
    }

    fn show_preview_window(&mut self, ctx: &egui::Context) {
        if !self.preview.open {
            return;
        }

        let mut open = self.preview.open;
        let title = tr!("preview-title");
        let fullscreen = self.preview.fullscreen;
        let window_id = egui::Id::new("preprint-preview");

        let mut window = egui::Window::new(&title).id(window_id).open(&mut open);
        if fullscreen {
            let screen = ctx
                .input(|i| i.raw.screen_rect)
                .unwrap_or(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                ));
            window = window
                .title_bar(false)
                .resizable(false)
                .fixed_pos(screen.min)
                .fixed_size(screen.size());
        } else {
            window = window
                .resizable(true)
                .default_size([960.0, 720.0])
                .min_width(560.0)
                .min_height(420.0);
        }
        window.show(ctx, |ui| {
            self.draw_preview_contents(ui, fullscreen);
        });
        self.preview.open = open;
    }

    fn draw_preview_contents(&mut self, ui: &mut egui::Ui, fullscreen: bool) {
        let p = palette(ui.ctx());
        ui.horizontal_wrapped(|ui| {
            let softproof_available = self.preview_softproof_texture.is_some();
            let softproof_label = if softproof_available {
                if self.preview.softproof_enabled {
                    format!("{}  {}", icons::EYE.as_str(), tr!("softproof-on"))
                } else {
                    format!("{}  {}", icons::EYE_SLASH.as_str(), tr!("softproof-off"))
                }
            } else {
                tr!("no-softproof-profile")
            };
            if ui
                .add_enabled(softproof_available, egui::Button::new(softproof_label))
                .clicked()
            {
                self.preview
                    .set_softproof_enabled(!self.preview.softproof_enabled());
            }
            let magnifier_label =
                format!("{}  {}", icons::MAGNIFYING_GLASS.as_str(), tr!("magnifier"));
            ui.toggle_value(&mut self.preview.magnifier_enabled, magnifier_label);
            if self.preview.magnifier_enabled {
                let zoom_label = tr!("zoom");
                let lens_label = tr!("lens");
                ui.add(
                    egui::Slider::new(
                        &mut self.preview.magnifier_zoom,
                        MIN_MAGNIFIER_ZOOM..=MAX_MAGNIFIER_ZOOM,
                    )
                    .text(zoom_label)
                    .suffix("\u{00D7}"),
                );
                ui.add(
                    egui::Slider::new(
                        &mut self.preview.magnifier_radius,
                        MIN_MAGNIFIER_RADIUS..=MAX_MAGNIFIER_RADIUS,
                    )
                    .text(lens_label),
                );
                self.preview.set_magnifier_zoom(self.preview.magnifier_zoom);
                self.preview
                    .set_magnifier_radius(self.preview.magnifier_radius);
                ui.label(
                    egui::RichText::new(tr!("magnifier-hint"))
                        .small()
                        .color(p.dim_text),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (fs_icon, fs_label) = if fullscreen {
                    (icons::ARROWS_IN.as_str(), tr!("exit-fullscreen"))
                } else {
                    (icons::ARROWS_OUT.as_str(), tr!("fullscreen"))
                };
                if ui.button(format!("{}  {}", fs_icon, fs_label)).clicked() {
                    self.preview.fullscreen = !self.preview.fullscreen;
                }
                if fullscreen
                    && ui
                        .button(format!("{}  {}", icons::X.as_str(), tr!("close")))
                        .clicked()
                {
                    self.preview.open = false;
                }
            });
        });

        ui.add_space(2.0);
        ui.add(egui::Separator::default().spacing(8.0));

        if !self.preview.compression_label().is_empty() {
            ui.label(
                egui::RichText::new(self.preview.compression_label().to_owned())
                    .small()
                    .color(p.dim_text),
            );
        }

        if self.preview.rendering {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    ui.add_space(10.0);
                    ui.add(
                        egui::ProgressBar::new(self.preview.progress())
                            .animate(true)
                            .desired_width(280.0)
                            .text(self.preview.progress_label().to_owned()),
                    );
                });
            });
            return;
        }

        let (texture_id, nearest_texture_id) = if self.preview.softproof_enabled() {
            (
                self.preview_softproof_texture
                    .as_ref()
                    .or(self.preview_base_texture.as_ref())
                    .map(|t| t.id()),
                self.preview_softproof_nearest_texture
                    .as_ref()
                    .or(self.preview_base_nearest_texture.as_ref())
                    .map(|t| t.id()),
            )
        } else {
            (
                self.preview_base_texture.as_ref().map(|t| t.id()),
                self.preview_base_nearest_texture.as_ref().map(|t| t.id()),
            )
        };

        let Some(image_size) = self.preview_image_size else {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new(tr!("no-preview-yet")).color(p.dim_text));
            });
            return;
        };

        let (Some(texture_id), Some(nearest_texture_id)) = (texture_id, nearest_texture_id) else {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new(tr!("no-preview-yet")).color(p.dim_text));
            });
            return;
        };

        self.draw_fitted_preview_image(ui, texture_id, nearest_texture_id, image_size);
    }

    fn draw_fitted_preview_image(
        &mut self,
        ui: &mut egui::Ui,
        texture_id: egui::TextureId,
        nearest_texture_id: egui::TextureId,
        image_size: [usize; 2],
    ) {
        draw_fitted_preview_image(
            ui,
            self.preview.magnifier_enabled(),
            self.preview.magnifier_radius(),
            self.preview.magnifier_zoom(),
            texture_id,
            nearest_texture_id,
            image_size,
        );
    }
}

impl eframe::App for PreprintApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        Self::configure_style(ui.ctx());
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);
        self.poll_batch();
        self.poll_preview(&ctx);

        egui::Frame::NONE
            .inner_margin(egui::Margin::same(18))
            .show(ui, |ui| {
                self.draw_top_bar(ui, &ctx);
                ui.add_space(6.0);
                ui.add(egui::Separator::default().spacing(6.0));
                ui.add_space(4.0);
                self.draw_status_strip(ui);
                ui.add_space(14.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.columns(2, |columns| {
                            columns[0].set_min_width(380.0);
                            columns[0].vertical(|ui| {
                                self.draw_file_card(ui, &ctx);
                                ui.add_space(12.0);
                                self.draw_batch_card(ui);
                            });

                            columns[1].vertical(|ui| {
                                self.draw_print_card(ui);
                                ui.add_space(12.0);
                                self.draw_output_card(ui);
                                ui.add_space(12.0);
                                self.draw_preview_summary(ui, &ctx);
                            });
                        });
                    });
            });

        self.show_preview_window(&ctx);
    }
}

#[derive(Clone, Debug)]
struct FileEntry {
    path: PathBuf,
    status: Option<FileStatus>,
}

impl FileEntry {
    fn status_label(&self) -> String {
        let Some(status) = &self.status else {
            return tr!("entry-inspecting");
        };

        if let Some(error) = &status.error {
            return tr!("entry-error", { error: error.as_str() });
        }

        match (status.dimensions, status.bit_depth) {
            (Some((width, height)), Some(depth)) => {
                let depth_label = depth.label();
                tr!(
                    "entry-dimensions-depth",
                    { width: width as i32, height: height as i32, depth: depth_label.as_str() }
                )
            }
            (Some((width, height)), None) => tr!(
                "entry-dimensions",
                { width: width as i32, height: height as i32 }
            ),
            _ => tr!("entry-ready"),
        }
    }
}

#[derive(Clone, Debug)]
struct FileStatus {
    dimensions: Option<(u32, u32)>,
    bit_depth: Option<SourceBitDepth>,
    error: Option<String>,
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

#[derive(Debug)]
enum PreviewMessage {
    Progress {
        request_id: u64,
        progress: f32,
        label: String,
    },
    Finished {
        request_id: u64,
        result: Result<PreviewImages, String>,
    },
}

#[derive(Debug)]
struct PreviewImages {
    base: DynamicImage,
    softproof: Option<DynamicImage>,
    compression_label: String,
}

struct BatchState {
    total: usize,
    completed: usize,
    running: bool,
    receiver: Receiver<BatchMessage>,
    results: Vec<BatchFileResult>,
}

#[derive(Debug)]
enum BatchMessage {
    File(BatchFileResult),
    Finished,
}

#[derive(Debug)]
struct BatchFileResult {
    input: PathBuf,
    output: Option<PathBuf>,
    error: Option<String>,
}

fn inspect_file(path: &Path) -> FileStatus {
    match load_image_metadata(path) {
        Ok(metadata) => FileStatus {
            dimensions: Some(metadata.dimensions),
            bit_depth: Some(metadata.bit_depth),
            error: None,
        },
        Err(error) => FileStatus {
            dimensions: None,
            bit_depth: None,
            error: Some(error.to_string()),
        },
    }
}

fn export_worker_count(job_count: usize) -> usize {
    if job_count == 0 {
        return 0;
    }

    let available = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);

    available.saturating_sub(1).max(1).min(4).min(job_count)
}

fn card(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    card_titled(ui, title, None, None::<fn(&mut egui::Ui)>, add_contents);
}

fn card_titled(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: Option<&str>,
    header_action: Option<impl FnOnce(&mut egui::Ui)>,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(16))
        .outer_margin(egui::Margin::same(0))
        .corner_radius(egui::CornerRadius::same(12))
        .fill(palette(ui.ctx()).card_bg)
        .stroke(egui::Stroke::new(1.0, palette(ui.ctx()).card_stroke))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(title.to_uppercase())
                            .size(13.5)
                            .strong()
                            .color(palette(ui.ctx()).text_secondary),
                    );
                    ui.add_space(6.0);
                    if let Some(sub) = subtitle {
                        ui.label(
                            egui::RichText::new(sub)
                                .small()
                                .color(palette(ui.ctx()).faint_text),
                        );
                    }
                    if let Some(action) = header_action {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), action);
                    }
                });
                ui.add_space(2.0);
                ui.add(egui::Separator::default().spacing(6.0));
                ui.add_space(8.0);
                add_contents(ui);
            });
        });
}

fn dot_separator(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("\u{00B7}")
            .color(palette(ui.ctx()).faint_text)
            .size(10.0),
    );
}

fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let (fill, stroke) = if enabled {
        (
            palette(ui.ctx()).accent,
            egui::Stroke::new(1.0, palette(ui.ctx()).accent_hover),
        )
    } else {
        (
            palette(ui.ctx()).accent_dim,
            egui::Stroke::new(1.0, palette(ui.ctx()).accent_dim),
        )
    };
    let text = egui::RichText::new(label)
        .color(egui::Color32::WHITE)
        .strong();
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(text)
            .fill(fill)
            .stroke(stroke)
            .corner_radius(egui::CornerRadius::same(8))
            .min_size(egui::vec2(96.0, 0.0)),
    );
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn draw_fitted_preview_image(
    ui: &mut egui::Ui,
    magnifier_enabled: bool,
    magnifier_radius: f32,
    magnifier_zoom: f32,
    texture_id: egui::TextureId,
    nearest_texture_id: egui::TextureId,
    image_size: [usize; 2],
) {
    let available = ui.available_size().max(egui::vec2(1.0, 1.0));
    let (canvas_rect, _) = ui.allocate_exact_size(available, egui::Sense::hover());
    let image_size = egui::vec2(image_size[0] as f32, image_size[1] as f32);
    let scale = (canvas_rect.width() / image_size.x)
        .min(canvas_rect.height() / image_size.y)
        .min(1.0)
        .max(0.01);
    let drawn_size = image_size * scale;
    let image_rect = egui::Rect::from_center_size(canvas_rect.center(), drawn_size);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

    ui.painter()
        .image(texture_id, image_rect, uv, egui::Color32::WHITE);

    let response = ui.interact(
        image_rect,
        ui.id().with("preview-image-magnifier"),
        egui::Sense::click_and_drag(),
    );

    if magnifier_enabled
        && response.contains_pointer()
        && ui.input(|input| input.pointer.primary_down())
        && let Some(center) = ui.input(|input| input.pointer.hover_pos())
    {
        paint_magnifier_lens(
            ui,
            nearest_texture_id,
            image_rect,
            center,
            magnifier_radius,
            magnifier_zoom,
        );
    }
}

fn paint_magnifier_lens(
    ui: &mut egui::Ui,
    texture_id: egui::TextureId,
    image_rect: egui::Rect,
    center: egui::Pos2,
    radius: f32,
    zoom: f32,
) {
    let radius = radius.clamp(MIN_MAGNIFIER_RADIUS, MAX_MAGNIFIER_RADIUS);
    let zoom = zoom.clamp(MIN_MAGNIFIER_ZOOM, MAX_MAGNIFIER_ZOOM);
    let segments = 64;
    let mut mesh = egui::Mesh::with_texture(texture_id);
    mesh.vertices.push(egui::epaint::Vertex {
        pos: center,
        uv: magnifier_uv(image_rect, center, center, zoom),
        color: egui::Color32::WHITE,
    });

    for index in 0..=segments {
        let angle = std::f32::consts::TAU * index as f32 / segments as f32;
        let offset = egui::vec2(angle.cos(), angle.sin()) * radius;
        let pos = center + offset;
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv: magnifier_uv(image_rect, center, pos, zoom),
            color: egui::Color32::WHITE,
        });
    }

    for index in 1..=segments {
        mesh.indices.push(0);
        mesh.indices.push(index as u32);
        mesh.indices.push(index as u32 + 1);
    }

    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter()
        .circle_stroke(center, radius, egui::Stroke::new(2.0, egui::Color32::WHITE));
    ui.painter().circle_stroke(
        center,
        radius + 1.0,
        egui::Stroke::new(1.0, egui::Color32::BLACK),
    );
}

fn magnifier_uv(
    image_rect: egui::Rect,
    lens_center: egui::Pos2,
    lens_pos: egui::Pos2,
    zoom: f32,
) -> egui::Pos2 {
    let source_pos = lens_center + (lens_pos - lens_center) / zoom;
    egui::pos2(
        ((source_pos.x - image_rect.left()) / image_rect.width()).clamp(0.0, 1.0),
        ((source_pos.y - image_rect.top()) / image_rect.height()).clamp(0.0, 1.0),
    )
}

fn build_processed_image(path: &Path, options: ProcessingOptions) -> Result<DynamicImage> {
    let loaded = load_image(path).context("failed to load image")?;
    add_border(&loaded.image, &options).context("failed to add border")
}

fn send_preview_progress(
    sender: &mpsc::Sender<PreviewMessage>,
    request_id: u64,
    progress: f32,
    label: impl Into<String>,
) {
    let _ = sender.send(PreviewMessage::Progress {
        request_id,
        progress,
        label: label.into(),
    });
}

fn export_one(
    input: &Path,
    output: &Path,
    processing: ProcessingOptions,
    export: ExportOptions,
) -> BatchFileResult {
    let result = build_processed_image(input, processing)
        .and_then(|image| save_image(&image, output, &export).context("failed to save image"));

    match result {
        Ok(()) => BatchFileResult {
            input: input.to_path_buf(),
            output: Some(output.to_path_buf()),
            error: None,
        },
        Err(error) => BatchFileResult {
            input: input.to_path_buf(),
            output: None,
            error: Some(error.to_string()),
        },
    }
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
            while reserved.contains(&output) {
                output = bump_reserved_path(&output);
            }
            reserved.insert(output.clone());
            (input.clone(), output)
        })
        .collect()
}

fn bump_reserved_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("image");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("png");

    for index in 1.. {
        let candidate = parent.join(format!("{stem}_{index}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!("unbounded output path search should always return")
}

fn dynamic_to_color_image(image: &DynamicImage) -> egui::ColorImage {
    let rgba = image.to_rgba8();
    egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    )
}

/// Downscale very large source images for the preview path so that expensive
/// operations (notably the `MirroredBlur` gaussian) stay interactive. Borders
/// scale proportionally, so the preview stays visually representative. The
/// export path is unaffected and always operates on the full-resolution image.
fn downscale_for_preview(image: DynamicImage) -> DynamicImage {
    const MAX_PREVIEW_DIM: u32 = 2400;
    let (width, height) = image.dimensions();
    let longest = width.max(height);
    if longest <= MAX_PREVIEW_DIM {
        return image;
    }
    let scale = MAX_PREVIEW_DIM as f32 / longest as f32;
    let new_width = ((width as f32) * scale).max(1.0) as u32;
    let new_height = ((height as f32) * scale).max(1.0) as u32;
    image.resize(new_width, new_height, image::imageops::FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidate_preview_clears_stale_result_state_and_worker() {
        crate::i18n::init();
        let (_sender, receiver) = mpsc::channel();
        let mut app = PreprintApp::default();
        let idle_label = app.preview.progress_label().to_owned();
        app.preview_request_id = 7;
        app.preview_receiver = Some(receiver);
        app.preview.open = true;
        app.preview.rendering = true;
        app.preview.set_progress(0.42, "Rendering preview");
        app.preview
            .set_compression_label("Compression preview: JPEG q80");
        app.preview_image_size = Some([100, 200]);

        app.invalidate_preview();

        assert_eq!(app.preview_request_id, 8);
        assert!(app.preview_receiver.is_none());
        assert!(!app.preview.open);
        assert!(!app.preview.rendering);
        assert_eq!(app.preview.progress(), 0.0);
        assert_eq!(app.preview.progress_label(), idle_label);
        assert!(app.preview.compression_label().is_empty());
        assert!(app.preview_image_size.is_none());
    }

    #[test]
    fn clear_preview_result_clears_metadata_without_invalidating_request() {
        let mut app = PreprintApp::default();
        app.preview_request_id = 11;
        app.preview
            .set_compression_label("Compression preview: lossless PNG");
        app.preview_image_size = Some([320, 240]);

        app.clear_preview_result();

        assert_eq!(app.preview_request_id, 11);
        assert!(app.preview.compression_label().is_empty());
        assert!(app.preview_image_size.is_none());
        assert!(app.preview_base_texture.is_none());
        assert!(app.preview_base_nearest_texture.is_none());
        assert!(app.preview_softproof_texture.is_none());
        assert!(app.preview_softproof_nearest_texture.is_none());
    }

    #[test]
    fn export_worker_count_is_bounded_for_memory_heavy_jobs() {
        assert_eq!(export_worker_count(0), 0);
        assert_eq!(export_worker_count(1), 1);
        assert!(export_worker_count(20) <= 4);
    }
}
