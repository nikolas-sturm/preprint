use rust_i18n::t;

pub const DEFAULT_LANGUAGE: &str = "en-US";

pub const LANGUAGES: &[(&str, &str)] = &[("en-US", "English"), ("de-DE", "Deutsch")];

pub fn init() {
    set_language(DEFAULT_LANGUAGE);
}

pub fn set_language(lang: &str) {
    rust_i18n::set_locale(lang);
}

pub fn current_language() -> String {
    rust_i18n::locale().to_string()
}

pub fn language_label(lang_id: &str) -> &str {
    LANGUAGES
        .iter()
        .find(|(id, _)| *id == lang_id)
        .map(|(_, label)| *label)
        .unwrap_or(lang_id)
}

pub fn translate(key: &str) -> String {
    t!(key).into_owned()
}

pub fn translate_for_locale(key: &str, locale: &str) -> String {
    t!(key, locale = locale).into_owned()
}

pub fn versioned(key: &str, version: &str) -> String {
    let locale = current_language();
    versioned_for_locale(key, version, &locale)
}

pub fn versioned_for_locale(key: &str, version: &str, locale: &str) -> String {
    t!(key, locale = locale, version = version).into_owned()
}

pub fn plural(key: &str, count: usize) -> String {
    let locale = current_language();
    plural_for_locale(key, count, &locale)
}

pub fn plural_for_locale(key: &str, count: usize, locale: &str) -> String {
    let form = if count == 1 { "singular" } else { "other" };
    let key = format!("{key}-{form}");
    t!(key, locale = locale, count = count).into_owned()
}
