#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 800.0])
            .with_min_inner_size([900.0, 600.0])
            .with_icon(load_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Preprint",
        options,
        Box::new(|cc| {
            register_fonts(&cc.egui_ctx);
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            preprint::i18n::init();
            Ok(Box::new(preprint::app::PreprintApp::new(&cc.egui_ctx)))
        }),
    )
}

fn register_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor_icons::add_fonts(&mut fonts);

    if let Some(proportional) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        proportional.push("phosphor-icons".to_owned());
    }
    if let Some(monospace) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        monospace.push("phosphor-icons".to_owned());
    }

    ctx.set_fonts(fonts);
}

fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/logo_220x220.png");
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .expect("failed to decode logo_220x220.png")
        .to_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}
