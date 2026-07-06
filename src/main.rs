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
            Ok(Box::<preprint::app::PreprintApp>::default())
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
    let size = 32usize;
    let accent = [96u8, 165, 250];
    let dark = [22u8, 24, 28];
    let white = [255u8, 255, 255];
    let mut rgba = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let on_frame = x >= 3 && x < size - 3 && y >= 3 && y < size - 3;
            let on_inner = x >= 8 && x < size - 8 && y >= 8 && y < size - 8;
            let rgb = if on_inner {
                white
            } else if on_frame {
                accent
            } else {
                dark
            };
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    }
}
