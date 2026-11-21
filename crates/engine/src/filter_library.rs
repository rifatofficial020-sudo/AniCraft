//! Filter presets (ADR 0091): named filter lists of the user's own, as Animate's Filters panel
//! keeps them (Save as Preset, apply, rename, delete), and the built-in ones (ADR 0099).
//!
//! The built-in presets are AniCraft's own, written here from the filters' parameters (no
//! other product's presets or values). They are always there: they can't be renamed, replaced
//! or deleted, only hidden from the menu, and a hidden one still applies by name, so a script
//! or a plug-in that names one works on every computer.
//!
//! Like the user's brushes (ADR 0068) the presets belong to the session, not the document: they
//! survive New and Open, the desktop app keeps them in its preferences and puts them back at
//! start (so the engine never reads the settings folder), and they move between computers as a
//! small JSON file (`filter.preset_export`/`_import`). Applying one copies its list onto the
//! instances: a preset edited or deleted later never changes artwork behind the artist's back.

use crate::{EngineError, Result};
use anicraft_doc::{Filter, FilterEntry, FilterSide, Rgba};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Most presets.
pub const MAX_PRESETS: usize = 200;
/// Longest preset name, in characters.
pub const MAX_NAME: usize = 64;
/// Most filters on one instance (and so in one preset).
pub const MAX_FILTERS: usize = 16;
/// Largest preset file read.
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// What a preset file says it is.
pub const FILE_FORMAT: &str = "anicraft-filter-presets";
pub const FILE_VERSION: u32 = 1;

/// A named filter list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilterPreset {
    pub name: String,
    #[serde(default)]
    pub filters: Vec<FilterEntry>,
}

impl FilterPreset {
    /// The preset with a clean name and its filters in range (at most [`MAX_FILTERS`]); `None`
    /// when the name is empty.
    pub fn sanitized(self) -> Option<FilterPreset> {
        let name = clean_name(&self.name)?;
        Some(FilterPreset { name, filters: self.filters.iter().take(MAX_FILTERS).map(FilterEntry::sanitized).collect() })
    }
}

/// A name as kept: one line, trimmed, at most [`MAX_NAME`] characters; `None` when empty.
pub fn clean_name(s: &str) -> Option<String> {
    let one: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let t: String = one.trim().chars().take(MAX_NAME).collect();
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// The built-in presets' names (English; the interface translates them where it draws them),
/// in the order the menu shows them.
pub const BUILTIN_NAMES: [&str; 8] = ["Soft Shadow", "Hard Shadow", "Neon Glow", "Outline", "Emboss", "Engrave", "Soft Focus", "Greyscale"];

/// The built-in presets (ADR 0099), in menu order. The values are AniCraft's own.
pub fn builtins() -> Vec<FilterPreset> {
    let shadow = |blur: f32, strength: f32, quality: u8, angle: f32, distance: f32| Filter::DropShadow { blur_x: blur, blur_y: blur, strength, quality, angle, distance, color: Rgba::BLACK, inner: false, knockout: false, hide_object: false };
    let glow = |blur: f32, strength: f32, color: u32| Filter::Glow { blur_x: blur, blur_y: blur, strength, quality: 3, color: Rgba::hex(color), inner: false, knockout: false };
    // A bevel lit from `angle`'s far side (45: light from the top left, so the edge looks raised;
    // 225: from the bottom right, so it looks cut in).
    let bevel = |angle: f32| Filter::Bevel { blur_x: 3.0, blur_y: 3.0, strength: 1.2, quality: 3, angle, distance: 2.0, shadow_color: Rgba::BLACK, highlight_color: Rgba::WHITE, side: FilterSide::Inner, knockout: false };
    let lists: [Vec<Filter>; 8] = [
        vec![shadow(12.0, 0.45, 3, 60.0, 6.0)],
        vec![shadow(0.0, 0.5, 1, 45.0, 5.0)],
        // A tight bright core inside a wide halo.
        vec![glow(4.0, 2.2, 0x9ff6ff), glow(22.0, 1.3, 0x1fb6ff)],
        // A small glow at full strength reads as a line round the artwork.
        vec![glow(3.0, 10.0, 0x000000)],
        vec![bevel(45.0)],
        vec![bevel(225.0)],
        vec![Filter::Blur { blur_x: 6.0, blur_y: 6.0, quality: 3 }],
        vec![Filter::AdjustColor { brightness: 0.0, contrast: 0.0, saturation: -100.0, hue: 0.0 }],
    ];
    BUILTIN_NAMES.iter().zip(lists).map(|(name, filters)| FilterPreset { name: (*name).to_string(), filters: filters.iter().map(|f| FilterEntry::from(f.sanitized())).collect() }).collect()
}

/// The built-in preset called `name` (case aside), as it is written.
pub fn builtin_name(name: &str) -> Option<&'static str> {
    BUILTIN_NAMES.into_iter().find(|n| same_name(n, name))
}

/// Whether `name` is a built-in preset's.
pub fn is_builtin_name(name: &str) -> bool {
    builtin_name(name).is_some()
}

/// The user's filter presets, in the order they were saved; names are unique, case aside, and
/// never a built-in preset's. With them, which built-in presets the user has hidden from the
/// menu (kept apart in the preferences, so the list of presets stays what older builds read).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FilterLibrary {
    presets: Vec<FilterPreset>,
    /// Built-in presets hidden from the menu, by name as [`BUILTIN_NAMES`] writes it.
    #[serde(skip)]
    hidden: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct PresetFile {
    format: String,
    version: u32,
    presets: Vec<Value>,
}

impl FilterLibrary {
    pub fn presets(&self) -> &[FilterPreset] {
        &self.presets
    }

    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }

    /// A library from untrusted entries (preferences, a file): bad entries skipped, values
    /// clamped, names made unique, at most [`MAX_PRESETS`].
    pub fn from_values(values: impl IntoIterator<Item = Value>) -> FilterLibrary {
        let mut lib = FilterLibrary::default();
        for v in values.into_iter().take(MAX_PRESETS * 4) {
            if let Ok(p) = serde_json::from_value::<FilterPreset>(v) {
                let _ = lib.add_unique(p);
            }
        }
        lib
    }

    /// The same library with every entry sanitized (after deserializing preferences).
    pub fn sanitized(self) -> FilterLibrary {
        let mut lib = FilterLibrary::from_values(self.presets.into_iter().filter_map(|p| serde_json::to_value(p).ok()));
        lib.restore_hidden(&self.hidden);
        lib
    }

    /// The built-in presets hidden from the menu.
    pub fn hidden(&self) -> &[String] {
        &self.hidden
    }

    pub fn is_hidden(&self, name: &str) -> bool {
        self.hidden.iter().any(|h| same_name(h, name))
    }

    /// The built-in presets the menu shows: all but the hidden ones.
    pub fn shown_builtins(&self) -> Vec<FilterPreset> {
        builtins().into_iter().filter(|p| !self.is_hidden(&p.name)).collect()
    }

    /// Sets which built-in presets are hidden, from the preferences: names that aren't a
    /// built-in preset's are dropped.
    pub fn restore_hidden(&mut self, names: &[String]) {
        self.hidden = BUILTIN_NAMES.into_iter().filter(|b| names.iter().take(MAX_PRESETS).any(|n| same_name(n, b))).map(str::to_string).collect();
    }

    /// Hides the built-in preset `name` from the menu, or shows it again. Its name as written.
    pub fn set_hidden(&mut self, name: &str, hidden: bool) -> Result<String> {
        let Some(builtin) = builtin_name(name) else {
            return Err(match self.find(name) {
                Some(p) => EngineError::Param(format!("'{}' is a preset of yours; only built-in presets are hidden (delete it instead)", p.name)),
                None => Self::missing(name),
            });
        };
        self.hidden.retain(|h| !same_name(h, builtin));
        if hidden {
            self.hidden.push(builtin.to_string());
            // In menu order, so the preferences don't change with the order of hiding.
            let kept = std::mem::take(&mut self.hidden);
            self.restore_hidden(&kept);
        }
        Ok(builtin.to_string())
    }

    /// The preset called `name`: one of the user's, or a built-in one (hidden or not).
    pub fn resolve(&self, name: &str) -> Option<FilterPreset> {
        self.find(name).cloned().or_else(|| builtins().into_iter().find(|p| same_name(&p.name, name)))
    }

    pub fn position(&self, name: &str) -> Option<usize> {
        self.presets.iter().position(|p| same_name(&p.name, name))
    }

    pub fn find(&self, name: &str) -> Option<&FilterPreset> {
        self.position(name).and_then(|i| self.presets.get(i))
    }

    fn missing(name: &str) -> EngineError {
        EngineError::Param(format!("you have no filter preset named '{}'", name.chars().take(MAX_NAME).collect::<String>()))
    }

    /// Saves `filters` under `name`, replacing the preset of that name. Returns the name kept
    /// and whether it replaced one.
    pub fn save(&mut self, name: &str, filters: &[FilterEntry]) -> Result<(String, bool)> {
        let p = FilterPreset { name: name.to_string(), filters: filters.to_vec() }.sanitized().ok_or_else(|| EngineError::Param("a filter preset needs a name".into()))?;
        if let Some(b) = builtin_name(&p.name) {
            return Err(EngineError::Param(format!("'{b}' is a built-in filter preset; save yours under another name")));
        }
        if p.filters.is_empty() {
            return Err(EngineError::State("there are no filters to save; add one first".into()));
        }
        let name = p.name.clone();
        if let Some(i) = self.position(&name)
            && let Some(slot) = self.presets.get_mut(i)
        {
            // The name as written now (the case may differ).
            *slot = p;
            return Ok((name, true));
        }
        if self.presets.len() >= MAX_PRESETS {
            return Err(EngineError::State(format!("you have {MAX_PRESETS} filter presets already; delete some first")));
        }
        self.presets.push(p);
        Ok((name, false))
    }

    /// Adds `p`, renaming it ("Glow 2", "Glow 3", ..) if the name is taken. Returns its name.
    pub fn add_unique(&mut self, p: FilterPreset) -> Result<String> {
        let p = p.sanitized().ok_or_else(|| EngineError::Param("a filter preset needs a name".into()))?;
        if self.presets.len() >= MAX_PRESETS {
            return Err(EngineError::State(format!("you have {MAX_PRESETS} filter presets already; delete some first")));
        }
        let base = p.name.clone();
        let mut name = base.clone();
        let mut k = 2;
        while self.position(&name).is_some() || is_builtin_name(&name) {
            if k > MAX_PRESETS + 10 {
                return Err(EngineError::State("too many filter presets of that name".into()));
            }
            let suffix = format!(" {k}");
            name = format!("{}{suffix}", base.chars().take(MAX_NAME.saturating_sub(suffix.chars().count())).collect::<String>().trim_end());
            k += 1;
        }
        self.presets.push(FilterPreset { name: name.clone(), ..p });
        Ok(name)
    }

    pub fn rename(&mut self, name: &str, to: &str) -> Result<String> {
        let i = self.position(name).ok_or_else(|| match builtin_name(name) {
            Some(b) => EngineError::Param(format!("'{b}' is a built-in filter preset; it keeps its name")),
            None => Self::missing(name),
        })?;
        let to = clean_name(to).ok_or_else(|| EngineError::Param("a filter preset needs a name".into()))?;
        if let Some(b) = builtin_name(&to) {
            return Err(EngineError::Param(format!("'{b}' is a built-in filter preset's name")));
        }
        if self.position(&to).is_some_and(|j| j != i) {
            return Err(EngineError::Param(format!("you already have a filter preset named '{to}'")));
        }
        if let Some(p) = self.presets.get_mut(i) {
            p.name = to.clone();
        }
        Ok(to)
    }

    pub fn delete(&mut self, name: &str) -> Result<FilterPreset> {
        let i = self.position(name).ok_or_else(|| match builtin_name(name) {
            Some(b) => EngineError::Param(format!("'{b}' is a built-in filter preset: it can't be deleted, only hidden (filter.preset_hide)")),
            None => Self::missing(name),
        })?;
        Ok(self.presets.remove(i))
    }

    /// A preset file holding `presets`.
    pub fn file_json(presets: &[FilterPreset]) -> String {
        let f = PresetFile { format: FILE_FORMAT.into(), version: FILE_VERSION, presets: presets.iter().filter_map(|p| serde_json::to_value(p).ok()).collect() };
        serde_json::to_string_pretty(&f).unwrap_or_default()
    }

    /// The presets in a preset file (bad entries skipped; values clamped).
    pub fn parse_file(text: &str) -> Result<Vec<FilterPreset>> {
        let f: PresetFile =
            serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| EngineError::Param(format!("not a filter preset file: {e}")))?;
        if f.format != FILE_FORMAT {
            return Err(EngineError::Param(format!("not a filter preset file (its format is '{}')", f.format.chars().take(40).collect::<String>())));
        }
        if f.version > FILE_VERSION {
            return Err(EngineError::Param(format!("this filter preset file is version {}, newer than this AniCraft reads ({FILE_VERSION})", f.version)));
        }
        Ok(f.presets.into_iter().take(MAX_PRESETS * 4).filter_map(|v| serde_json::from_value::<FilterPreset>(v).ok()).filter_map(FilterPreset::sanitized).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_doc::Filter;

    fn blur(b: f32) -> FilterEntry {
        Filter::Blur { blur_x: b, blur_y: b, quality: 1 }.into()
    }

    #[test]
    fn saving_renaming_and_deleting() {
        let mut lib = FilterLibrary::default();
        assert_eq!(lib.save("  Soft \n glow ", &[blur(4.0)]).unwrap(), ("Soft   glow".to_string(), false));
        assert!(lib.save("soft   GLOW", &[blur(8.0), blur(1e9)]).unwrap().1, "same name, case aside, replaces");
        assert_eq!(lib.presets().len(), 1);
        assert_eq!(lib.presets()[0].filters[1], blur(anicraft_doc::MAX_BLUR), "values clamped");
        assert!(lib.save(" ", &[blur(1.0)]).is_err());
        assert!(lib.save("Empty", &[]).is_err(), "nothing to save");
        assert_eq!(lib.add_unique(FilterPreset { name: "soft   glow".into(), filters: vec![blur(1.0)] }).unwrap(), "soft   glow 2");
        assert_eq!(lib.rename("SOFT   GLOW 2", "Haze").unwrap(), "Haze");
        assert!(lib.rename("Haze", "Soft   Glow").is_err() && lib.rename("nope", "x").is_err() && lib.rename("Haze", "  ").is_err());
        assert_eq!(lib.rename("haze", "HAZE").unwrap(), "HAZE", "a new case of its own name");
        assert_eq!(lib.find("haze").map(|p| p.filters.len()), Some(1));
        assert_eq!(lib.delete("haze").unwrap().name, "HAZE");
        assert!(lib.delete("haze").is_err());
        let long = "x".repeat(200);
        assert_eq!(lib.save(&long, &[blur(1.0)]).unwrap().0.chars().count(), MAX_NAME);
    }

    #[test]
    fn preset_files_round_trip_and_reject_junk() {
        let off = FilterEntry { filter: Filter::new("glow").unwrap(), enabled: false };
        let mine = vec![FilterPreset { name: "A".into(), filters: vec![blur(3.0), off] }, FilterPreset { name: "B".into(), filters: vec![Filter::new("bevel").unwrap().into()] }];
        let text = FilterLibrary::file_json(&mine);
        assert!(text.contains("\"enabled\": false"), "a switched-off filter keeps its switch: {text}");
        assert_eq!(FilterLibrary::parse_file(&text).unwrap(), mine);
        assert_eq!(FilterLibrary::parse_file(&format!("\u{feff}{text}")).unwrap(), mine);
        assert!(FilterLibrary::parse_file("{}").is_err());
        assert!(FilterLibrary::parse_file(r#"{"format": "anicraft-brushes", "version": 1, "presets": []}"#).is_err());
        assert!(FilterLibrary::parse_file(r#"{"format": "anicraft-filter-presets", "version": 99, "presets": []}"#).is_err());
        // Bad entries are skipped, values clamped, bare filters take their defaults.
        let got = FilterLibrary::parse_file(r#"{"format": "anicraft-filter-presets", "version": 1, "presets": [7, {"name": ""}, {"name": "x", "filters": [{"type": "sparkle"}]}, {"name": "Old", "filters": [{"type": "blur", "blur_x": -4}, {"type": "glow"}]}]}"#).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].filters[0], Filter::Blur { blur_x: 0.0, blur_y: 5.0, quality: 1 }.into());
        assert!(matches!(got[0].filters[1], FilterEntry { filter: Filter::Glow { blur_x: 5.0, .. }, enabled: true }));
        // Preferences: the library as a list, names made unique.
        let lib = FilterLibrary::from_values(serde_json::from_str::<Vec<Value>>(&serde_json::to_string(&[mine[0].clone(), mine[0].clone()]).unwrap()).unwrap());
        assert_eq!(lib.presets().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["A", "A 2"]);
        assert_eq!(lib.clone().sanitized(), lib);
    }
}
