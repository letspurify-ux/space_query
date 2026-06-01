use fltk::{app, enums::Font};
use std::collections::HashSet;

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

pub fn profile_by_name(name: &str) -> FontProfile {
    if let Some(profile) = FONT_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.name.eq_ignore_ascii_case(name))
    {
        return profile;
    }

    if let Some(font) = find_font_by_name(name) {
        return FontProfile {
            name: "Custom",
            normal: font,
            bold: font,
            italic: font,
        };
    }

    FONT_PROFILES[0]
}

pub fn available_font_names() -> Vec<String> {
    let mut names: Vec<String> = FONT_PROFILES
        .iter()
        .map(|profile| profile.name.to_string())
        .collect();

    for raw_name in app::get_font_names() {
        let normalized = normalize_font_name(&raw_name);
        if !normalized.is_empty() {
            names.push(normalized);
        }
    }

    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for name in names {
        let key = name.to_ascii_lowercase();
        if seen.insert(key) {
            deduped.push(name);
        }
    }

    let mut defaults = Vec::new();
    let mut extras = Vec::new();
    for name in deduped {
        if FONT_PROFILES
            .iter()
            .any(|profile| profile.name.eq_ignore_ascii_case(&name))
        {
            defaults.push(name);
        } else {
            extras.push(name);
        }
    }
    extras.sort_by_key(|name| name.to_ascii_lowercase());
    defaults.extend(extras);
    defaults
}

pub fn configured_editor_profile() -> FontProfile {
    let config = AppConfig::load();
    profile_by_name(&config.editor_font)
}

pub fn configured_ui_font_size() -> i32 {
    let config = AppConfig::load();
    config.ui_font_size.clamp(8, 24) as i32
}

fn find_font_by_name(name: &str) -> Option<Font> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    for (idx, raw_name) in app::get_font_names().into_iter().enumerate() {
        if raw_name.eq_ignore_ascii_case(name)
            || normalize_font_name(&raw_name).eq_ignore_ascii_case(name)
        {
            return Some(Font::by_index(idx));
        }
    }

    None
}

fn normalize_font_name(raw_name: &str) -> String {
    let trimmed = raw_name.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut chars = trimmed.chars();
    let Some(prefix) = chars.next() else {
        return trimmed.to_string();
    };
    let rest = chars.as_str().trim();

    if rest.is_empty() {
        return trimmed.to_string();
    }

    if prefix == ' ' {
        return rest.to_string();
    }

    if matches!(prefix, 'B' | 'I' | 'P') {
        let starts_upper = rest.chars().next().map(char::is_uppercase).unwrap_or(false);
        if starts_upper {
            return rest.to_string();
        }
    }

    trimmed.to_string()
}
