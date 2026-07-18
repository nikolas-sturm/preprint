use preprint::i18n::{
    LANGUAGES, language_label, plural_for_locale, translate_for_locale, versioned_for_locale,
};

#[test]
fn exposes_supported_language_labels() {
    assert_eq!(LANGUAGES, &[("en-US", "English"), ("de-DE", "Deutsch")]);
    assert_eq!(language_label("de-DE"), "Deutsch");
    assert_eq!(language_label("fr-FR"), "fr-FR");
}

#[test]
fn translates_both_locales_without_changing_global_locale() {
    assert_eq!(
        translate_for_locale("tagline", "en-US"),
        "Prepare photos for print"
    );
    assert_eq!(
        translate_for_locale("tagline", "de-DE"),
        "Fotos für den Druck vorbereiten"
    );
    assert_eq!(translate_for_locale("unit-inches", "en-US"), "Inches");
    assert_eq!(translate_for_locale("unit-inches", "de-DE"), "Zoll");
    assert_eq!(translate_for_locale("resize-fit", "en-US"), "Fit and pad");
    assert_eq!(
        translate_for_locale("resize-fit", "de-DE"),
        "Einpassen und auffüllen"
    );
}

#[test]
fn selects_singular_and_other_messages_without_global_locale() {
    assert_eq!(
        plural_for_locale("files-loaded", 1, "en-US"),
        "1 file loaded"
    );
    assert_eq!(
        plural_for_locale("files-loaded", 2, "en-US"),
        "2 files loaded"
    );
    assert_eq!(
        plural_for_locale("loaded-images", 1, "de-DE"),
        "1 Bild geladen"
    );
    assert_eq!(
        plural_for_locale("export-started", 3, "de-DE"),
        "Export mit gespeicherten Einstellungen gestartet — 3 Worker. Änderungen gelten für den nächsten Stapel."
    );
}

#[test]
fn interpolates_update_version_for_each_locale() {
    assert_eq!(
        versioned_for_locale("update-to-version", "1.2.3", "en-US"),
        "Update to 1.2.3"
    );
    assert_eq!(
        versioned_for_locale("update-to-version", "1.2.3", "de-DE"),
        "Auf 1.2.3 aktualisieren"
    );
}
