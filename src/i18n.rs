pub const DEFAULT_LANGUAGE: &str = "en-US";

pub const LANGUAGES: &[(&str, &str)] = &[("en-US", "English"), ("de-DE", "Deutsch")];

pub fn init() {
    let en = include_str!("../i18n/en-US.ftl");
    let de = include_str!("../i18n/de-DE.ftl");

    egui_i18n::set_use_isolating(false);
    egui_i18n::set_fallback(DEFAULT_LANGUAGE);
    egui_i18n::set_language(DEFAULT_LANGUAGE);

    egui_i18n::load_translations_from_text("en-US", en).expect("failed to load en-US translations");
    egui_i18n::load_translations_from_text("de-DE", de).expect("failed to load de-DE translations");
}

pub fn set_language(lang: &str) {
    egui_i18n::set_language(lang);
}

pub fn current_language() -> String {
    egui_i18n::get_language()
}

pub fn language_label(lang_id: &str) -> &str {
    LANGUAGES
        .iter()
        .find(|(id, _)| *id == lang_id)
        .map(|(_, label)| *label)
        .unwrap_or(lang_id)
}
