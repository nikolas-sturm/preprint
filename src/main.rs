#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui::{App, AppContext as _, Application, WindowBounds, WindowOptions, px, size};
use gpui_component::{Root, Theme, ThemeMode};

fn main() {
    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            let (preferences, preferences_error) = match preprint::preferences::load() {
                Ok(preferences) => (preferences, None),
                Err(error) => (
                    preprint::preferences::Preferences::default(),
                    Some(error.to_string()),
                ),
            };
            let theme = match preferences.theme {
                preprint::preferences::ThemePreference::Light => ThemeMode::Light,
                preprint::preferences::ThemePreference::Dark => ThemeMode::Dark,
            };
            Theme::change(theme, None, cx);
            preprint::i18n::set_language(&preferences.language);

            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::centered(size(px(1180.), px(800.)), cx)),
                    window_min_size: Some(size(px(900.), px(600.))),
                    app_id: Some("dev.preprint.app".into()),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("Preprint".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let workflow = preferences.workflow.clone();
                    let preferences_error = preferences_error.clone();
                    let app = cx.new(|cx| {
                        preprint::app::PreprintApp::new(workflow, preferences_error, window, cx)
                    });
                    #[cfg(windows)]
                    app.update(cx, |app, cx| app.check_for_updates(cx));
                    cx.new(|cx| Root::new(app, window, cx))
                },
            )
            .expect("failed to open Preprint window");
            cx.activate(true);
        });
}
