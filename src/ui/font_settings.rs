use fltk::{app, enums::Font};
use std::collections::HashSet;
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::sync::{OnceLock, RwLock};

use crate::utils::AppConfig;

#[derive(Clone, Copy)]
pub struct FontProfile {
    pub name: &'static str,
    pub normal: Font,
    pub bold: Font,
    pub italic: Font,
}

pub const FONT_PROFILES: &[FontProfile] = &[
    FontProfile {
        name: "Courier",
        normal: Font::Courier,
        bold: Font::CourierBold,
        italic: Font::CourierItalic,
    },
    FontProfile {
        name: "Helvetica",
        normal: Font::Helvetica,
        bold: Font::HelveticaBold,
        italic: Font::HelveticaItalic,
    },
    FontProfile {
        name: "Times",
        normal: Font::Times,
        bold: Font::TimesBold,
        italic: Font::TimesItalic,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FontStyle {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

#[derive(Clone)]
struct FontCatalogEntry {
    display_name: String,
    raw_name: String,
    font: Font,
    style: FontStyle,
}

struct FontCatalog {
    entries: Vec<FontCatalogEntry>,
}

#[derive(Clone, Copy)]
struct RuntimeFontSettings {
    editor_profile: FontProfile,
    result_profile: FontProfile,
    ui_size: i32,
    editor_size: u32,
    result_size: u32,
}

static FONT_CATALOG: OnceLock<FontCatalog> = OnceLock::new();
static RUNTIME_FONT_SETTINGS: OnceLock<RwLock<RuntimeFontSettings>> = OnceLock::new();
static ORIGINAL_DEFAULT_FONT_NAME: OnceLock<CString> = OnceLock::new();

unsafe extern "C" {
    #[link_name = "Fl_set_font"]
    fn fl_set_font_from_slot(target: c_int, source: c_int);
    #[link_name = "Fl_set_font2"]
    fn fl_set_font_from_name(target: c_int, name: *const c_char);
}

/// Remaps only FLTK's default font slot, preserving the selected font's own slot.
/// Uses FLTK's slot-copy API for selected fonts so runtime switching neither
/// swaps the selected slot nor allocates a retained font-name string.
pub fn apply_global_default_font(font: Font) {
    let original_default = ORIGINAL_DEFAULT_FONT_NAME
        .get_or_init(|| persistent_font_name(app::get_font(Font::Helvetica)));
    let desired_name = if font == Font::Helvetica {
        original_default.to_string_lossy().into_owned()
    } else {
        app::get_font(font)
    };

    if app::get_font(Font::Helvetica) != desired_name {
        // SAFETY: both functions are part of FLTK's public C API. Font slot 0
        // is valid, `font.bits()` comes from FLTK, and the CString is held in a
        // OnceLock for the full process lifetime as required by Fl::set_font.
        unsafe {
            if font == Font::Helvetica {
                fl_set_font_from_name(Font::Helvetica.bits(), original_default.as_ptr());
            } else {
                fl_set_font_from_slot(Font::Helvetica.bits(), font.bits());
            }
        }
    }
}

fn persistent_font_name(name: String) -> CString {
    let mut bytes = name.into_bytes();
    bytes.retain(|byte| *byte != 0);
    if bytes.is_empty() {
        bytes.extend_from_slice(b"Helvetica");
    }
    // SAFETY: every interior NUL was removed above. CString appends the final
    // terminator itself and owns this buffer for its full lifetime.
    unsafe { CString::from_vec_unchecked(bytes) }
}

fn font_catalog() -> &'static FontCatalog {
    // `App::load_system_fonts()` has already populated this process-local list
    // at startup; avoid asking the OS to enumerate the same fonts again.
    FONT_CATALOG.get_or_init(|| FontCatalog::from_raw_names(app::fonts()))
}

impl FontCatalog {
    fn from_raw_names(raw_names: Vec<String>) -> Self {
        let regular_prefixed_names = raw_names
            .iter()
            .filter_map(|raw_name| raw_name.strip_prefix(' '))
            .map(|name| name.trim().to_lowercase())
            .filter(|name| !name.is_empty())
            .collect::<HashSet<_>>();

        let entries = raw_names
            .into_iter()
            .enumerate()
            // Slot 0 is the remappable application default and is not a stable
            // identity for a system font.
            .skip(1)
            .filter_map(|(index, raw_name)| {
                let (display_name, style) =
                    parse_raw_font_name(&raw_name, &regular_prefixed_names)?;
                Some(FontCatalogEntry {
                    display_name,
                    raw_name,
                    font: Font::by_index(index),
                    style,
                })
            })
            .collect();
        Self { entries }
    }

    fn entry_by_name(&self, name: &str) -> Option<&FontCatalogEntry> {
        let name = name.trim();
        self.entries
            .iter()
            .filter(|entry| {
                entry.display_name.eq_ignore_ascii_case(name)
                    || entry.raw_name.eq_ignore_ascii_case(name)
            })
            .min_by_key(|entry| match entry.style {
                FontStyle::Regular => 0,
                FontStyle::Bold => 1,
                FontStyle::Italic => 2,
                FontStyle::BoldItalic => 3,
            })
    }

    fn family_style(&self, display_name: &str, style: FontStyle) -> Option<Font> {
        self.entries
            .iter()
            .find(|entry| {
                entry.display_name.eq_ignore_ascii_case(display_name) && entry.style == style
            })
            .map(|entry| entry.font)
    }

    fn named_variant(&self, display_name: &str, style: FontStyle) -> Option<Font> {
        variant_name_candidates(display_name, style)
            .into_iter()
            .find_map(|candidate| self.entry_by_name(&candidate).map(|entry| entry.font))
    }
}

fn parse_raw_font_name(
    raw_name: &str,
    regular_prefixed_names: &HashSet<String>,
) -> Option<(String, FontStyle)> {
    if let Some(regular_name) = raw_name.strip_prefix(' ') {
        let display_name = regular_name.trim();
        return (!display_name.is_empty()).then(|| (display_name.to_string(), FontStyle::Regular));
    }

    let trimmed = raw_name.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut chars = raw_name.chars();
    let prefix = chars.next()?;
    let rest = chars.as_str().trim();
    let style = match prefix {
        'B' => FontStyle::Bold,
        'I' => FontStyle::Italic,
        'P' => FontStyle::BoldItalic,
        _ => FontStyle::Regular,
    };
    if style != FontStyle::Regular
        && !rest.is_empty()
        && regular_prefixed_names.contains(&rest.to_lowercase())
    {
        Some((rest.to_string(), style))
    } else {
        // macOS/Pango/Wayland return complete face names. A genuine name such
        // as PTMono or ITFDevanagari must never lose its first character.
        Some((trimmed.to_string(), FontStyle::Regular))
    }
}

fn variant_name_candidates(name: &str, style: FontStyle) -> Vec<String> {
    let suffixes: &[&str] = match style {
        FontStyle::Bold => &["Bold", "Semibold", "DemiBold"],
        FontStyle::Italic => &["Italic", "Oblique"],
        _ => return Vec::new(),
    };
    let trimmed = name.trim();
    let base = [" Regular", "-Regular"]
        .into_iter()
        .find_map(|suffix| trimmed.strip_suffix(suffix))
        .unwrap_or(trimmed);
    let mut candidates = Vec::with_capacity(suffixes.len() * 2);
    for suffix in suffixes {
        candidates.push(format!("{base} {suffix}"));
        candidates.push(format!("{base}-{suffix}"));
    }
    candidates
}

pub fn profile_by_name(name: &str) -> FontProfile {
    if let Some(profile) = FONT_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.name.eq_ignore_ascii_case(name.trim()))
    {
        return profile;
    }

    let catalog = font_catalog();
    if let Some(entry) = catalog.entry_by_name(name) {
        let normal = catalog
            .family_style(&entry.display_name, FontStyle::Regular)
            .unwrap_or(entry.font);
        let bold = catalog
            .family_style(&entry.display_name, FontStyle::Bold)
            .or_else(|| catalog.named_variant(&entry.display_name, FontStyle::Bold))
            .unwrap_or(normal);
        let italic = catalog
            .family_style(&entry.display_name, FontStyle::Italic)
            .or_else(|| catalog.named_variant(&entry.display_name, FontStyle::Italic))
            .unwrap_or(normal);
        return FontProfile {
            name: "Custom",
            normal,
            bold,
            italic,
        };
    }

    FONT_PROFILES[0]
}

pub fn resolved_font_name(name: &str) -> String {
    if let Some(profile) = FONT_PROFILES
        .iter()
        .find(|profile| profile.name.eq_ignore_ascii_case(name.trim()))
    {
        return profile.name.to_string();
    }
    font_catalog()
        .entry_by_name(name)
        .map(|entry| entry.display_name.clone())
        .unwrap_or_else(|| FONT_PROFILES[0].name.to_string())
}

pub fn available_font_names() -> Vec<String> {
    let mut names = FONT_PROFILES
        .iter()
        .map(|profile| profile.name.to_string())
        .collect::<Vec<_>>();
    names.extend(
        font_catalog()
            .entries
            .iter()
            .map(|entry| entry.display_name.clone()),
    );

    let mut seen = HashSet::new();
    names.retain(|name| seen.insert(name.to_lowercase()));
    let default_count = FONT_PROFILES.len().min(names.len());
    names[default_count..].sort_by_key(|name| name.to_lowercase());
    names
}

fn runtime_font_settings() -> RuntimeFontSettings {
    *RUNTIME_FONT_SETTINGS
        .get_or_init(|| {
            let config = AppConfig::runtime();
            RwLock::new(RuntimeFontSettings::from_config(&config))
        })
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl RuntimeFontSettings {
    fn from_config(config: &AppConfig) -> Self {
        Self {
            editor_profile: profile_by_name(&config.editor_font),
            result_profile: profile_by_name(&config.result_font),
            ui_size: config.normalized_ui_font_size() as i32,
            editor_size: config.normalized_editor_font_size(),
            result_size: config.normalized_result_font_size(),
        }
    }
}

pub fn update_runtime_font_settings(config: &AppConfig) {
    AppConfig::update_runtime(config);
    let settings = RuntimeFontSettings::from_config(config);
    let runtime = RUNTIME_FONT_SETTINGS.get_or_init(|| RwLock::new(settings));
    *runtime
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = settings;
}

pub fn configured_editor_profile() -> FontProfile {
    runtime_font_settings().editor_profile
}

pub fn configured_result_profile() -> FontProfile {
    runtime_font_settings().result_profile
}

pub fn configured_ui_font_size() -> i32 {
    runtime_font_settings().ui_size
}

pub fn configured_editor_font_size() -> u32 {
    runtime_font_settings().editor_size
}

pub fn configured_result_font_size() -> u32 {
    runtime_font_settings().result_size
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regular_names(names: &[&str]) -> HashSet<String> {
        names
            .iter()
            .filter_map(|name| name.strip_prefix(' '))
            .map(|name| name.trim().to_lowercase())
            .collect()
    }

    #[test]
    fn prefixed_fltk_style_names_preserve_the_family() {
        let names = [
            " IBM Plex Mono",
            "BIBM Plex Mono",
            "IIBM Plex Mono",
            "PIBM Plex Mono",
        ];
        let regular = regular_names(&names);

        assert_eq!(
            parse_raw_font_name(names[0], &regular),
            Some(("IBM Plex Mono".to_string(), FontStyle::Regular))
        );
        assert_eq!(
            parse_raw_font_name(names[1], &regular),
            Some(("IBM Plex Mono".to_string(), FontStyle::Bold))
        );
        assert_eq!(
            parse_raw_font_name(names[2], &regular),
            Some(("IBM Plex Mono".to_string(), FontStyle::Italic))
        );
        assert_eq!(
            parse_raw_font_name(names[3], &regular),
            Some(("IBM Plex Mono".to_string(), FontStyle::BoldItalic))
        );
    }

    #[test]
    fn complete_font_names_starting_with_style_letters_are_not_truncated() {
        let regular = HashSet::new();
        for name in ["PTMono-Regular", "ITFDevanagari-Bold", "BIZ UDPGothic"] {
            assert_eq!(
                parse_raw_font_name(name, &regular),
                Some((name.to_string(), FontStyle::Regular))
            );
        }
    }

    #[test]
    fn catalog_groups_prefixed_styles_without_collapsing_complete_names() {
        let catalog = FontCatalog::from_raw_names(vec![
            "slot-zero".to_string(),
            " IBM Plex Mono".to_string(),
            "BIBM Plex Mono".to_string(),
            "PTMono-Regular".to_string(),
        ]);

        assert_eq!(
            catalog
                .entry_by_name("IBM Plex Mono")
                .map(|entry| entry.style),
            Some(FontStyle::Regular)
        );
        assert!(catalog
            .family_style("IBM Plex Mono", FontStyle::Bold)
            .is_some());
        assert_eq!(
            catalog
                .entry_by_name("PTMono-Regular")
                .map(|entry| entry.display_name.as_str()),
            Some("PTMono-Regular")
        );
    }

    #[test]
    fn face_variant_candidates_replace_regular_suffixes() {
        assert!(variant_name_candidates("PTMono-Regular", FontStyle::Bold)
            .contains(&"PTMono-Bold".to_string()));
        assert!(variant_name_candidates("Menlo", FontStyle::Italic)
            .contains(&"Menlo Italic".to_string()));
    }

    #[test]
    fn persistent_font_name_removes_nul_without_panicking() {
        assert_eq!(persistent_font_name("A\0B".to_string()).to_str(), Ok("AB"));
        assert_eq!(
            persistent_font_name("\0".to_string()).to_str(),
            Ok("Helvetica")
        );
    }
}
