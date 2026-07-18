use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
};

use crate::i18n::{DEFAULT_LANGUAGE, LANGUAGES};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemePreference {
    Light,
    #[default]
    Dark,
}

impl ThemePreference {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LengthUnit {
    Millimeters,
    #[default]
    Centimeters,
    Inches,
}

impl LengthUnit {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "mm" => Some(Self::Millimeters),
            "cm" => Some(Self::Centimeters),
            "in" => Some(Self::Inches),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Millimeters => "mm",
            Self::Centimeters => "cm",
            Self::Inches => "in",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowPreferences {
    pub print_width_cm: f32,
    pub print_height_cm: f32,
    pub border_mm: f32,
    pub length_unit: LengthUnit,
    pub resize_mode: String,
    pub target_ppi: u32,
    pub border_style: String,
    pub output_format: String,
    pub quality: u8,
    pub bit_depth: u8,
    pub png_compression: u8,
    pub tiff_compression: String,
    pub tiff_deflate_level: String,
    pub softproof_profile: Option<PathBuf>,
    pub convert_output_profile: bool,
    pub output_dir: Option<PathBuf>,
}

impl Default for WorkflowPreferences {
    fn default() -> Self {
        Self {
            print_width_cm: 60.0,
            print_height_cm: 40.0,
            border_mm: 8.0,
            length_unit: LengthUnit::Centimeters,
            resize_mode: "none".to_owned(),
            target_ppi: 300,
            border_style: "mirrored-blur".to_owned(),
            output_format: "tiff".to_owned(),
            quality: 90,
            bit_depth: 16,
            png_compression: 6,
            tiff_compression: "deflate".to_owned(),
            tiff_deflate_level: "best".to_owned(),
            softproof_profile: None,
            convert_output_profile: false,
            output_dir: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preferences {
    pub language: String,
    pub theme: ThemePreference,
    pub workflow: WorkflowPreferences,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            language: DEFAULT_LANGUAGE.to_owned(),
            theme: ThemePreference::Dark,
            workflow: WorkflowPreferences::default(),
        }
    }
}

pub fn load() -> io::Result<Preferences> {
    let path = preferences_path().ok_or_else(|| {
        io::Error::new(
            ErrorKind::NotFound,
            "could not determine user configuration directory",
        )
    })?;
    match load_from(&path) {
        Ok(preferences) => Ok(preferences),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Preferences::default()),
        Err(error) => Err(error),
    }
}

pub fn save_language(language: &str) -> io::Result<()> {
    update_preferences(|preferences| {
        if is_supported_language(language) {
            preferences.language = language.to_owned();
        }
    })
}

pub fn save_theme(theme: ThemePreference) -> io::Result<()> {
    update_preferences(|preferences| preferences.theme = theme)
}

pub fn save_workflow(workflow: WorkflowPreferences) -> io::Result<()> {
    update_preferences(|preferences| preferences.workflow = workflow)
}

fn update_preferences(update: impl FnOnce(&mut Preferences)) -> io::Result<()> {
    let path = preferences_path().ok_or_else(|| {
        io::Error::new(
            ErrorKind::NotFound,
            "could not determine user configuration directory",
        )
    })?;
    update_at(&path, update)
}

fn update_at(path: &Path, update: impl FnOnce(&mut Preferences)) -> io::Result<()> {
    let _lock = PreferencesLock::acquire(path)?;
    let mut preferences = match load_from(path) {
        Ok(preferences) => preferences,
        Err(error) if error.kind() == ErrorKind::NotFound => Preferences::default(),
        Err(error) => return Err(error),
    };
    update(&mut preferences);
    save_to(path, &preferences)
}

struct PreferencesLock {
    file: File,
}

impl PreferencesLock {
    fn acquire(preferences_path: &Path) -> io::Result<Self> {
        if let Some(parent) = preferences_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let path = preferences_path.with_extension("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.lock()?;
        Ok(Self { file })
    }
}

impl Drop for PreferencesLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn load_from(path: &Path) -> io::Result<Preferences> {
    let content = fs::read_to_string(path)?;
    let mut preferences = Preferences::default();
    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "language" if is_supported_language(value.trim()) => {
                preferences.language = value.trim().to_owned();
            }
            "theme" => {
                if let Some(theme) = ThemePreference::parse(value.trim()) {
                    preferences.theme = theme;
                }
            }
            "print_width_cm" => {
                if let Some(value) = parse_f32(value, 0.1, 200.0) {
                    preferences.workflow.print_width_cm = value;
                }
            }
            "print_height_cm" => {
                if let Some(value) = parse_f32(value, 0.1, 200.0) {
                    preferences.workflow.print_height_cm = value;
                }
            }
            "border_mm" => {
                if let Some(value) = parse_f32(value, 0.0, 200.0) {
                    preferences.workflow.border_mm = value;
                }
            }
            "length_unit" => {
                if let Some(value) = LengthUnit::parse(value.trim()) {
                    preferences.workflow.length_unit = value;
                }
            }
            "resize_mode" if matches!(value.trim(), "none" | "fit" | "fill") => {
                preferences.workflow.resize_mode = value.trim().to_owned();
            }
            "target_ppi" => {
                if let Some(value) = parse_u32(value, 1, 9600) {
                    preferences.workflow.target_ppi = value;
                }
            }
            "border_style" if matches!(value.trim(), "white" | "black" | "mirrored-blur") => {
                preferences.workflow.border_style = value.trim().to_owned();
            }
            "output_format" if matches!(value.trim(), "png" | "jpeg" | "tiff") => {
                preferences.workflow.output_format = value.trim().to_owned();
            }
            "quality" => {
                if let Some(value) = parse_u8(value, 1, 100) {
                    preferences.workflow.quality = value;
                }
            }
            "bit_depth" => {
                if let Some(value) = parse_u8(value, 8, 16).filter(|value| matches!(value, 8 | 16))
                {
                    preferences.workflow.bit_depth = value;
                }
            }
            "png_compression" => {
                if let Some(value) = parse_u8(value, 1, 9) {
                    preferences.workflow.png_compression = value;
                }
            }
            "tiff_compression" if matches!(value.trim(), "lzw" | "deflate") => {
                preferences.workflow.tiff_compression = value.trim().to_owned();
            }
            "tiff_deflate_level" if matches!(value.trim(), "fast" | "balanced" | "best") => {
                preferences.workflow.tiff_deflate_level = value.trim().to_owned();
            }
            "convert_output_profile" if matches!(value.trim(), "true" | "false") => {
                preferences.workflow.convert_output_profile = value.trim() == "true";
            }
            "softproof_profile_hex" if !value.trim().is_empty() => {
                if let Some(path) = decode_path(value.trim()) {
                    preferences.workflow.softproof_profile = Some(path);
                }
            }
            "output_dir" if !value.trim().is_empty() => {
                preferences.workflow.output_dir = Some(PathBuf::from(value.trim()));
            }
            "output_dir_hex" if !value.trim().is_empty() => {
                if let Some(path) = decode_path(value.trim()) {
                    preferences.workflow.output_dir = Some(path);
                }
            }
            _ => {}
        }
    }
    Ok(preferences)
}

fn parse_f32(value: &str, min: f32, max: f32) -> Option<f32> {
    value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && (min..=max).contains(value))
}

fn parse_u8(value: &str, min: u8, max: u8) -> Option<u8> {
    value
        .trim()
        .parse::<u8>()
        .ok()
        .filter(|value| (min..=max).contains(value))
}

fn parse_u32(value: &str, min: u32, max: u32) -> Option<u32> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| (min..=max).contains(value))
}

#[cfg(unix)]
fn encode_path(path: &Path) -> String {
    encode_hex(path.as_os_str().as_bytes())
}

#[cfg(unix)]
fn decode_path(value: &str) -> Option<PathBuf> {
    decode_hex(value)
        .map(std::ffi::OsString::from_vec)
        .map(PathBuf::from)
}

#[cfg(windows)]
fn encode_path(path: &Path) -> String {
    let bytes = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_be_bytes)
        .collect::<Vec<_>>();
    encode_hex(&bytes)
}

#[cfg(windows)]
fn decode_path(value: &str) -> Option<PathBuf> {
    let bytes = decode_hex(value)?;
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let wide = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    Some(PathBuf::from(std::ffi::OsString::from_wide(&wide)))
}

#[cfg(not(any(unix, windows)))]
fn encode_path(path: &Path) -> String {
    encode_hex(path.to_string_lossy().as_bytes())
}

#[cfg(not(any(unix, windows)))]
fn decode_path(value: &str) -> Option<PathBuf> {
    String::from_utf8(decode_hex(value)?)
        .ok()
        .map(PathBuf::from)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

fn save_to(path: &Path, preferences: &Preferences) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let workflow = &preferences.workflow;
    let output_dir = workflow
        .output_dir
        .as_deref()
        .map_or_else(String::new, encode_path);
    let softproof_profile = workflow
        .softproof_profile
        .as_deref()
        .map_or_else(String::new, encode_path);
    let content = format!(
        concat!(
            "language={}\n",
            "theme={}\n",
            "print_width_cm={}\n",
            "print_height_cm={}\n",
            "border_mm={}\n",
            "length_unit={}\n",
            "resize_mode={}\n",
            "target_ppi={}\n",
            "border_style={}\n",
            "output_format={}\n",
            "quality={}\n",
            "bit_depth={}\n",
            "png_compression={}\n",
            "tiff_compression={}\n",
            "tiff_deflate_level={}\n",
            "softproof_profile_hex={}\n",
            "convert_output_profile={}\n",
            "output_dir_hex={}\n"
        ),
        preferences.language,
        preferences.theme.as_str(),
        workflow.print_width_cm,
        workflow.print_height_cm,
        workflow.border_mm,
        workflow.length_unit.as_str(),
        workflow.resize_mode,
        workflow.target_ppi,
        workflow.border_style,
        workflow.output_format,
        workflow.quality,
        workflow.bit_depth,
        workflow.png_compression,
        workflow.tiff_compression,
        workflow.tiff_deflate_level,
        softproof_profile,
        workflow.convert_output_profile,
        output_dir,
    );
    let mut temporary = tempfile::Builder::new()
        .prefix(".preferences-")
        .tempfile_in(path.parent().unwrap_or_else(|| Path::new(".")))?;
    temporary.write_all(content.as_bytes())?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| error.error)
}

fn is_supported_language(language: &str) -> bool {
    LANGUAGES.iter().any(|(id, _)| *id == language)
}

fn preferences_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("Preprint").join("preferences.conf"));
    }

    #[cfg(target_os = "macos")]
    {
        return env::var_os("HOME").map(PathBuf::from).map(|path| {
            path.join("Library")
                .join("Application Support")
                .join("Preprint")
                .join("preferences.conf")
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
            return Some(
                PathBuf::from(path)
                    .join("preprint")
                    .join("preferences.conf"),
            );
        }
        return env::var_os("HOME").map(PathBuf::from).map(|path| {
            path.join(".config")
                .join("preprint")
                .join("preferences.conf")
        });
    }

    #[allow(unreachable_code)]
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".preprint").join("preferences.conf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.conf");
        let preferences = Preferences {
            language: "de-DE".to_owned(),
            theme: ThemePreference::Light,
            workflow: WorkflowPreferences {
                print_width_cm: 29.7,
                print_height_cm: 21.0,
                border_mm: 5.0,
                length_unit: LengthUnit::Millimeters,
                resize_mode: "fill".to_owned(),
                target_ppi: 360,
                border_style: "white".to_owned(),
                output_format: "png".to_owned(),
                quality: 82,
                bit_depth: 8,
                png_compression: 9,
                tiff_compression: "lzw".to_owned(),
                tiff_deflate_level: "fast".to_owned(),
                softproof_profile: Some(directory.path().join("printer.icc")),
                convert_output_profile: true,
                output_dir: Some(directory.path().join("exports")),
            },
        };

        save_to(&path, &preferences).unwrap();

        assert_eq!(load_from(&path).unwrap(), preferences);
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.conf");
        fs::write(&path, "language=unknown\ntheme=neon\n").unwrap();

        assert_eq!(load_from(&path).unwrap(), Preferences::default());
    }

    #[test]
    fn legacy_preferences_keep_workflow_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.conf");
        fs::write(&path, "language=de-DE\ntheme=light\n").unwrap();

        let preferences = load_from(&path).unwrap();

        assert_eq!(preferences.language, "de-DE");
        assert_eq!(preferences.theme, ThemePreference::Light);
        assert_eq!(preferences.workflow, WorkflowPreferences::default());
    }

    #[test]
    fn invalid_workflow_values_keep_field_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.conf");
        fs::write(
            &path,
            concat!(
                "print_width_cm=-1\n",
                "print_height_cm=nan\n",
                "border_mm=999\n",
                "length_unit=points\n",
                "resize_mode=stretch\n",
                "target_ppi=0\n",
                "quality=0\n",
                "bit_depth=12\n",
                "png_compression=20\n"
            ),
        )
        .unwrap();

        assert_eq!(
            load_from(&path).unwrap().workflow,
            WorkflowPreferences::default()
        );
    }

    #[test]
    fn update_does_not_overwrite_unreadable_existing_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.conf");
        let invalid = [0xff, 0xfe, 0xfd];
        fs::write(&path, invalid).unwrap();

        assert!(
            update_at(&path, |preferences| preferences.theme =
                ThemePreference::Light)
            .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), invalid);
    }

    #[test]
    fn concurrent_updates_preserve_unrelated_fields() {
        use std::{
            sync::{Arc, Barrier},
            thread,
        };

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.conf");
        let barrier = Arc::new(Barrier::new(2));
        let language_path = path.clone();
        let language_barrier = barrier.clone();
        let language = thread::spawn(move || {
            language_barrier.wait();
            update_at(&language_path, |preferences| {
                preferences.language = "de-DE".to_owned();
            })
            .unwrap();
        });
        let theme = thread::spawn(move || {
            barrier.wait();
            update_at(&path, |preferences| {
                preferences.theme = ThemePreference::Light;
            })
            .unwrap();
        });

        language.join().unwrap();
        theme.join().unwrap();
        let preferences = load_from(&directory.path().join("preferences.conf")).unwrap();
        assert_eq!(preferences.language, "de-DE");
        assert_eq!(preferences.theme, ThemePreference::Light);
    }

    #[cfg(unix)]
    #[test]
    fn output_directory_round_trips_non_utf8_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.conf");
        let output = PathBuf::from(std::ffi::OsString::from_vec(vec![b'o', 0xff, b'k']));
        let mut preferences = Preferences::default();
        preferences.workflow.output_dir = Some(output.clone());

        save_to(&path, &preferences).unwrap();

        assert_eq!(load_from(&path).unwrap().workflow.output_dir, Some(output));
    }
}
