//! Themes of the user's own (ADR 0117): small JSON files in the settings folder's `themes`
//! folder, each naming colours for the palette's roles over one of the built-in palettes.
//!
//! A theme is data only: role names and colours, never widget styles, sizes or code. A file is
//! read whole and checked whole (the format, every role name, every colour, and the contrast its
//! colours reach, computed by the same rules as the built-in palettes); a file that fails any
//! check is refused with a message naming what failed, and nothing of it is applied.

use crate::theme::{COLOUR_ROLES, Palette, Shortfall, Standard, TRANSLUCENT_ROLES, Tokens};
use std::path::{Path, PathBuf};

/// The format this build reads and writes (`"anicraft_theme": 1`).
pub const FORMAT: u64 = 1;
/// A theme file is a few kilobytes; anything larger is not one.
pub const MAX_BYTES: u64 = 64 * 1024;
/// Most themes listed from the folder, and most folder entries looked at to find them.
pub const MAX_THEMES: usize = 200;
const MAX_ENTRIES: usize = 10_000;
/// Longest name and author a theme shows.
pub const MAX_NAME: usize = 64;

/// A theme as its file gave it, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct UserTheme {
    /// The name the file gives itself, if it does.
    pub name: Option<String>,
    pub author: Option<String>,
    /// The built-in palette it starts from: roles the file doesn't name, the egui base look,
    /// the title bar, and whether controls carry outlines.
    pub base: Palette,
    pub tokens: Tokens,
    /// Pairs below WCAG AA that don't stop the theme (blocks against the grid, control edges).
    pub notes: Vec<Shortfall>,
}

/// Why a theme file isn't used.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    Unreadable(String),
    TooLarge,
    NotJson(String),
    /// JSON, but not a theme (no `anicraft_theme`).
    NotATheme,
    /// Written for a newer format than this build reads.
    Newer(u64),
    UnknownKey(String),
    BadText(&'static str),
    BadBase(String),
    UnknownRole(String),
    BadColour { role: String, value: String },
    /// A role that must be solid was given a see-through colour.
    NotSolid(String),
    /// The colours read the other way round from the base palette (`dark`: what the base is).
    WrongBase { dark: bool },
    /// The first pair below the contrast it needs, and how many more there are.
    Contrast { first: Shortfall, more: usize },
}

impl Refusal {
    /// The reason, in the interface language, naming the key, role or pair at fault.
    pub fn message(&self) -> String {
        match self {
            Refusal::Unreadable(e) => tr!("The file could not be read: {e}", e = e),
            Refusal::TooLarge => tr!("The file is larger than {size} KB, so it is not a theme.", size = MAX_BYTES / 1024),
            Refusal::NotJson(e) => tr!("The file is not valid JSON: {e}", e = e),
            Refusal::NotATheme => tr!("The file has no \"anicraft_theme\" number, so it is not a theme.").into(),
            Refusal::Newer(v) => tr!("The theme is in format {theirs}; this AniCraft reads format {ours}. Update AniCraft to use it.", theirs = v, ours = FORMAT),
            Refusal::UnknownKey(k) => tr!("\"{key}\" is not part of a theme file.", key = k),
            Refusal::BadText(k) => tr!("\"{key}\" must be a short line of text, up to {max} characters.", key = k, max = MAX_NAME),
            Refusal::BadBase(b) => tr!("The base \"{base}\" is not one of \"dark\", \"light\" or \"high-contrast\".", base = b),
            Refusal::UnknownRole(r) => tr!("There is no colour role named \"{role}\".", role = r),
            Refusal::BadColour { role, value } => tr!("The colour of {role}, {value}, is not written as \"#rrggbb\" or \"#rrggbbaa\".", role = role, value = value),
            Refusal::NotSolid(r) => tr!("{role} must be a solid colour (\"#rrggbb\"): text and surfaces can't be see-through.", role = r),
            Refusal::WrongBase { dark: true } => tr!("The text is darker than the panels, but the base is dark. Use \"base\": \"light\".").into(),
            Refusal::WrongBase { dark: false } => tr!("The text is lighter than the panels, but the base is light. Use \"base\": \"dark\".").into(),
            Refusal::Contrast { first, more: 0 } => tr!("{pair} has a contrast of {ratio}:1; it needs {needs}:1.", pair = first.pair, ratio = ratio(first.ratio), needs = first.needs),
            Refusal::Contrast { first, more } => {
                trn!("{pair} has a contrast of {ratio}:1; it needs {needs}:1. {n} more pair is too low.", "{pair} has a contrast of {ratio}:1; it needs {needs}:1. {n} more pairs are too low.", *more, pair = first.pair, ratio = ratio(first.ratio), needs = first.needs)
            }
        }
    }
}

/// A contrast ratio as shown: two decimals, rounded down so 4.499 never reads as 4.50.
fn ratio(r: f32) -> String {
    format!("{:.2}", (r * 100.0).floor() / 100.0)
}

/// The name a base palette has in a theme file.
pub fn base_name(p: Palette) -> &'static str {
    match p {
        Palette::Graphite => "dark",
        Palette::Light => "light",
        Palette::HighContrast => "high-contrast",
    }
}

fn base_named(name: &str) -> Option<Palette> {
    match name {
        "dark" | "graphite" => Some(Palette::Graphite),
        "light" => Some(Palette::Light),
        "high-contrast" => Some(Palette::HighContrast),
        _ => None,
    }
}

/// `#rrggbb` or `#rrggbbaa`, as red, green, blue, opacity.
fn parse_hex(s: &str) -> Option<[u8; 4]> {
    let hex = s.strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| hex.get(i * 2..i * 2 + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    Some([byte(0)?, byte(1)?, byte(2)?, if hex.len() == 8 { byte(3)? } else { 255 }])
}

fn hex(c: egui::Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
}

/// A short line of text (a name, an author): trimmed, not empty, no control characters.
fn short_text(v: &serde_json::Value, key: &'static str) -> Result<String, Refusal> {
    let s = v.as_str().map(str::trim).ok_or(Refusal::BadText(key))?;
    if s.is_empty() || s.chars().count() > MAX_NAME || s.chars().any(char::is_control) {
        return Err(Refusal::BadText(key));
    }
    Ok(s.to_string())
}

/// Reads and checks a theme file's text. All or nothing: `Ok` only when every part is good.
pub fn parse(text: &str) -> Result<UserTheme, Refusal> {
    if text.len() as u64 > MAX_BYTES {
        return Err(Refusal::TooLarge);
    }
    let json: serde_json::Value = serde_json::from_str(text).map_err(|e| Refusal::NotJson(e.to_string()))?;
    let file = json.as_object().ok_or(Refusal::NotATheme)?;
    match file.get("anicraft_theme").and_then(serde_json::Value::as_u64) {
        None | Some(0) => return Err(Refusal::NotATheme),
        Some(v) if v > FORMAT => return Err(Refusal::Newer(v)),
        Some(_) => {}
    }
    if let Some(key) = file.keys().find(|k| !matches!(k.as_str(), "anicraft_theme" | "name" | "author" | "base" | "colors")) {
        return Err(Refusal::UnknownKey(key.chars().take(MAX_NAME).collect()));
    }
    let name = file.get("name").map(|v| short_text(v, "name")).transpose()?;
    let author = file.get("author").map(|v| short_text(v, "author")).transpose()?;
    let base = match file.get("base") {
        None => Palette::Graphite,
        Some(v) => v.as_str().and_then(base_named).ok_or_else(|| Refusal::BadBase(v.as_str().map_or_else(|| v.to_string(), str::to_string).chars().take(MAX_NAME).collect()))?,
    };
    let mut tokens = *base.tokens();
    let empty = serde_json::Map::new();
    let colours = match file.get("colors") {
        None => &empty,
        Some(v) => v.as_object().ok_or_else(|| Refusal::UnknownKey("colors".into()))?,
    };
    for (role, value) in colours {
        if !COLOUR_ROLES.contains(&role.as_str()) {
            return Err(Refusal::UnknownRole(role.chars().take(MAX_NAME).collect()));
        }
        let bad = || Refusal::BadColour { role: role.clone(), value: value.to_string().chars().take(MAX_NAME).collect() };
        let [r, g, b, a] = value.as_str().and_then(parse_hex).ok_or_else(bad)?;
        if a != 255 && !TRANSLUCENT_ROLES.contains(&role.as_str()) {
            return Err(Refusal::NotSolid(role.clone()));
        }
        tokens.set_colour(role, crate::theme::role_colour(r, g, b, a));
    }
    // An accent bar is flat unless the theme gives it another end: a theme that changes only
    // the accent must not get a gradient from its base's.
    if !colours.contains_key("accent_end") {
        tokens.accent_end = tokens.accent;
    }
    if crate::theme::reads_as_dark(&tokens) != tokens.dark {
        return Err(Refusal::WrongBase { dark: tokens.dark });
    }
    let (required, notes): (Vec<Shortfall>, Vec<Shortfall>) = crate::theme::shortfalls(&tokens, Standard::Aa).into_iter().partition(|s| s.required);
    let more = required.len().saturating_sub(1);
    if let Some(first) = required.into_iter().next() {
        return Err(Refusal::Contrast { first, more });
    }
    Ok(UserTheme { name, author, base, tokens, notes })
}

/// A theme file holding every colour role of `tokens`, to start a theme from (Export Current
/// Theme). Reading it back gives the same tokens over the same base.
pub fn export(name: &str, base: Palette, tokens: &Tokens) -> String {
    let quoted = |s: &str| serde_json::Value::String(s.to_string()).to_string();
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    let mut out = String::from("{\n");
    out.push_str(&format!("  \"anicraft_theme\": {FORMAT},\n")); // i18n-exempt: the file format
    out.push_str(&format!("  \"name\": {},\n", quoted(if name.is_empty() { "Theme" } else { &name }))); // i18n-exempt: the file format
    out.push_str(&format!("  \"base\": {},\n", quoted(base_name(base)))); // i18n-exempt: the file format
    out.push_str("  \"colors\": {\n"); // i18n-exempt: the file format
    let lines: Vec<String> = COLOUR_ROLES.iter().filter_map(|role| tokens.colour(role).map(|c| format!("    {}: {}", quoted(role), quoted(&hex(c))))).collect();
    out.push_str(&lines.join(",\n"));
    out.push_str("\n  }\n}\n");
    out
}

/// One theme file in the folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The file's name without `.json`: what the preferences remember.
    pub id: String,
    pub path: PathBuf,
    pub theme: Result<UserTheme, Refusal>,
}

impl Entry {
    /// The name shown in the list: the file's own, or its file name.
    pub fn name(&self) -> &str {
        self.theme.as_ref().ok().and_then(|t| t.name.as_deref()).unwrap_or(&self.id)
    }
}

/// The themes in the user's folder, as last read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    pub entries: Vec<Entry>,
    /// Theme files beyond `MAX_THEMES`, not listed.
    pub left_out: usize,
}

impl Library {
    /// Reads every `.json` file in `dir` (not its subfolders), in file-name order. A folder that
    /// isn't there is an empty library.
    pub fn scan(dir: &Path) -> Library {
        let Ok(listing) = std::fs::read_dir(dir) else { return Library::default() };
        let mut paths: Vec<PathBuf> = listing.take(MAX_ENTRIES).filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")) && p.is_file()).collect();
        paths.sort();
        let left_out = paths.len().saturating_sub(MAX_THEMES);
        paths.truncate(MAX_THEMES);
        let entries = paths.into_iter().map(|path| Entry { id: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), theme: read(&path), path }).collect();
        Library { entries, left_out }
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// How many themes can be used, and how many were refused.
    pub fn counts(&self) -> (usize, usize) {
        let good = self.entries.iter().filter(|e| e.theme.is_ok()).count();
        (good, self.entries.len() - good)
    }
}

/// Reads and checks one theme file.
pub fn read(path: &Path) -> Result<UserTheme, Refusal> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| Refusal::Unreadable(e.to_string()))?;
    let mut bytes = Vec::new();
    // One byte past the limit is enough to know it is too large.
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| Refusal::Unreadable(e.to_string()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Refusal::TooLarge);
    }
    let text = String::from_utf8(bytes).map_err(|e| Refusal::NotJson(e.to_string()))?;
    // A byte-order mark some editors write is not part of the JSON.
    parse(text.strip_prefix('\u{feff}').unwrap_or(&text))
}

/// A file name in `dir` that isn't taken: `stem.json`, then `stem-2.json`, `stem-3.json`…
pub fn free_path(dir: &Path, stem: &str) -> Option<PathBuf> {
    (1..=999u32).map(|n| dir.join(if n == 1 { format!("{stem}.json") } else { format!("{stem}-{n}.json") })).find(|p| !p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{GRAPHITE, HIGH_CONTRAST, LIGHT, T};

    const BUILT_IN: [(Palette, Tokens); 3] = [(Palette::Graphite, GRAPHITE), (Palette::Light, LIGHT), (Palette::HighContrast, HIGH_CONTRAST)];

    fn theme(body: &str) -> Result<UserTheme, Refusal> {
        parse(&format!(r#"{{"anicraft_theme": 1, {body}}}"#))
    }

    #[test]
    fn a_small_theme_changes_only_the_roles_it_names() {
        let t = theme(r##""name": " Plum ", "author": "Sam", "base": "dark", "colors": {"accent": "#9A3CE0", "span": "#2ad1b2", "stage_shadow": "#00000080"}"##).expect("a good theme");
        assert_eq!((t.name.as_deref(), t.author.as_deref(), t.base), (Some("Plum"), Some("Sam"), Palette::Graphite));
        let mut want = GRAPHITE;
        want.accent = crate::theme::role_colour(0x9a, 0x3c, 0xe0, 255);
        want.accent_end = want.accent;
        want.span = crate::theme::role_colour(0x2a, 0xd1, 0xb2, 255);
        want.stage_shadow = crate::theme::role_colour(0, 0, 0, 0x80);
        assert_eq!(t.tokens, want, "every other role is the base's");
        // No colours at all: the base itself, under another name. No base: dark.
        assert_eq!(theme(r#""name": "Plain""#).map(|t| (t.base, t.tokens)), Ok((Palette::Graphite, GRAPHITE)));
        assert_eq!(theme(r#""base": "light""#).map(|t| t.tokens), Ok(LIGHT));
        assert_eq!(theme(r#""base": "high-contrast""#).map(|t| t.tokens), Ok(HIGH_CONTRAST));
    }

    #[test]
    fn an_accent_bar_is_flat_unless_the_theme_gives_it_another_end() {
        let flat = theme(r##""colors": {"accent": "#9a3ce0"}"##).expect("flat");
        assert_eq!(flat.tokens.accent_end, flat.tokens.accent);
        let graded = theme(r##""colors": {"accent": "#9a3ce0", "accent_end": "#d6336c"}"##).expect("graded");
        assert_ne!(graded.tokens.accent_end, graded.tokens.accent);
    }

    #[test]
    fn files_that_are_not_themes_are_refused_with_the_reason() {
        assert!(matches!(parse("not json"), Err(Refusal::NotJson(_))));
        assert_eq!(parse("[1, 2]"), Err(Refusal::NotATheme));
        assert_eq!(parse(r#"{"name": "x"}"#), Err(Refusal::NotATheme));
        assert_eq!(parse(r#"{"anicraft_theme": "1"}"#), Err(Refusal::NotATheme));
        assert_eq!(parse(r#"{"anicraft_theme": 0}"#), Err(Refusal::NotATheme));
        assert_eq!(parse(r#"{"anicraft_theme": 2}"#), Err(Refusal::Newer(2)));
        assert_eq!(theme(r#""widgets": {}"#), Err(Refusal::UnknownKey("widgets".into())));
        assert_eq!(theme(r#""colors": []"#), Err(Refusal::UnknownKey("colors".into())));
        assert_eq!(theme(r#""name": """#), Err(Refusal::BadText("name")));
        assert_eq!(theme(r#""name": 7"#), Err(Refusal::BadText("name")));
        assert_eq!(theme(&format!(r#""author": "{}""#, "a".repeat(MAX_NAME + 1))), Err(Refusal::BadText("author")));
        assert_eq!(theme(r#""name": "two\nlines""#), Err(Refusal::BadText("name")));
        assert_eq!(theme(r#""base": "sepia""#), Err(Refusal::BadBase("sepia".into())));
        assert_eq!(theme(r#""base": 3"#), Err(Refusal::BadBase("3".into())));
        assert_eq!(theme(r##""colors": {"button_fill": "#ffffff"}"##), Err(Refusal::UnknownRole("button_fill".into())));
        for bad in [r#""red""#, r##""#fff""##, r##""#12345g""##, r##""#1234567""##, "12", "null", r##""#+1+2+3""##] {
            assert!(matches!(theme(&format!(r#""colors": {{"panel": {bad}}}"#)), Err(Refusal::BadColour { ref role, .. }) if role == "panel"), "{bad}");
        }
        assert_eq!(theme(r##""colors": {"panel": "#1b1b1f80"}"##), Err(Refusal::NotSolid("panel".into())));
        assert_eq!(parse(&" ".repeat(MAX_BYTES as usize + 1)), Err(Refusal::TooLarge));
        // Every refusal says why, naming what is at fault.
        assert!(Refusal::UnknownRole("button_fill".into()).message().contains("button_fill"));
        assert!(Refusal::Newer(9).message().contains('9'));
    }

    #[test]
    fn a_theme_below_wcag_aa_is_refused_naming_the_roles() {
        // Grey text on the dark panels: 2.6:1 where text needs 4.5:1.
        let low = theme(r##""colors": {"text_dim": "#666670"}"##);
        let Err(Refusal::Contrast { first, more }) = &low else { panic!("{low:?}") };
        assert_eq!((first.pair.as_str(), first.needs, first.required), ("text_dim on panel", 4.5, true));
        assert!(first.ratio < 4.5 && first.ratio > 2.0, "computed from the colours: {}", first.ratio);
        assert!(*more >= 3, "and on the other surfaces: {more}");
        let said = low.as_ref().map(|_| ()).map_err(Refusal::message);
        assert!(said.as_ref().is_err_and(|m| m.contains("text_dim on panel") && m.contains("4.5")), "{said:?}");
        // Just under and just over the line, by the numbers.
        assert!(crate::theme::contrast(crate::theme::role_colour(0x76, 0x76, 0x76, 255), egui::Color32::WHITE) >= 4.5);
        assert!(crate::theme::contrast(crate::theme::role_colour(0x77, 0x77, 0x77, 255), egui::Color32::WHITE) < 4.5);
        // The focus ring must show; a faint block on the grid is only noted.
        assert!(matches!(theme(r##""colors": {"accent": "#24242a"}"##), Err(Refusal::Contrast { first, .. }) if first.pair.starts_with("on_fill on accent") || first.pair.starts_with("accent on")));
        let noted = theme(r##""colors": {"control_edge": "#2a2a30"}"##).expect("applied, with notes");
        assert!(noted.notes.iter().any(|n| n.pair == "control_edge on panel" && !n.required), "{:?}", noted.notes);
        // Light text needs a dark base, and the other way round: said, not guessed at.
        assert_eq!(theme(r##""base": "light", "colors": {"text": "#ffffff", "panel": "#101014"}"##), Err(Refusal::WrongBase { dark: false }));
        assert_eq!(theme(r##""colors": {"text": "#101014", "panel": "#f0f0f3"}"##), Err(Refusal::WrongBase { dark: true }));
        // A two-tone icon fill that swallows the outline is refused; a soft one is fine.
        assert!(matches!(theme(r##""colors": {"icon_accent": "#b4b4bc"}"##), Err(Refusal::Contrast { first, .. }) if first.pair.contains("icon_accent")));
        assert!(theme(r##""colors": {"icon_accent": "#2e66f066"}"##).is_ok());
    }

    #[test]
    fn exporting_a_palette_and_reading_it_back_gives_the_same_tokens() {
        for (palette, tokens) in BUILT_IN {
            let text = export("Mine", palette, &tokens);
            let back = parse(&text).unwrap_or_else(|e| panic!("{palette:?}: {}", e.message()));
            assert_eq!((back.base, back.name.as_deref()), (palette, Some("Mine")));
            assert_eq!(back.tokens, tokens, "{palette:?} survives the round trip bit for bit");
            assert!(COLOUR_ROLES.iter().all(|role| text.contains(&format!("\"{role}\": \"#"))), "every role is written");
            assert_eq!(export("Mine", back.base, &back.tokens), text, "and writes the same file again");
        }
        // A changed theme too, with a see-through role and a hostile name.
        let mut mine = theme(r##""colors": {"accent": "#9a3ce0", "accent_end": "#d6336c", "icon_accent": "#9a3ce04d", "cell_tint": "#ffffff10"}"##).expect("good");
        mine.name = Some("A \"quoted\" \\ name".into());
        let back = parse(&export(mine.name.as_deref().unwrap_or(""), mine.base, &mine.tokens)).expect("reads back");
        assert_eq!((back.tokens, back.name), (mine.tokens, mine.name));
        assert!(parse(&export("\n", Palette::Light, &LIGHT)).is_ok(), "an empty name still writes a theme");
    }

    /// The built-in palettes, pinned: adding user themes (ADR 0117) changed none of their colours,
    /// and nothing may change one by accident. The sums are over every role's four bytes, in order.
    #[test]
    fn the_built_in_palettes_are_unchanged_bit_for_bit() {
        fn sum(t: &Tokens) -> u64 {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for role in COLOUR_ROLES.iter().filter(|r| !matches!(**r, "icon_accent" | "accent_end")) {
                for byte in t.colour(role).map(|c| c.to_array()).unwrap_or_default() {
                    h = (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
                }
            }
            h
        }
        assert_eq!([sum(&GRAPHITE), sum(&LIGHT), sum(&HIGH_CONTRAST)], [18055272439377304835, 17569161940681386332, 4288341231507623258]);
        // The roles this ADR added draw nothing new in them: no icon fill, a flat accent bar.
        for (palette, tokens) in BUILT_IN {
            assert_eq!((tokens.icon_accent.a(), tokens.accent_end), (0, tokens.accent), "{palette:?}");
            assert_eq!(*palette.tokens(), tokens);
        }
        // A user's theme, applied and taken off again, leaves them as they were.
        let ctx = egui::Context::default();
        let plum = theme(r##""base": "light", "colors": {"accent": "#7a1fd0"}"##).expect("good");
        crate::theme::apply_user_theme(&ctx, plum.base, plum.tokens);
        assert!(crate::theme::user_theme_active());
        assert_eq!((T.accent, T.panel, crate::theme::palette()), (plum.tokens.accent, LIGHT.panel, Palette::Light));
        assert_eq!(ctx.global_style().visuals.selection.stroke.color, plum.tokens.accent, "egui's widgets are restyled with it");
        for (palette, tokens) in BUILT_IN {
            crate::theme::apply_palette(&ctx, palette);
            assert!(!crate::theme::user_theme_active());
            assert_eq!(*T, tokens, "{palette:?}");
        }
        crate::theme::apply_palette(&ctx, Palette::Graphite);
    }

    #[test]
    fn every_colour_in_the_tokens_is_a_role_a_theme_can_set() {
        // The fields of `Tokens` that hold a colour, read from its source: none is missed.
        let src = include_str!("theme.rs");
        let body = src.split("pub struct Tokens {").nth(1).and_then(|s| s.split("\n}\n").next()).unwrap_or("");
        let fields: Vec<&str> = body.lines().filter_map(|l| l.trim().strip_prefix("pub ")?.strip_suffix(": Color32,")).collect();
        assert_eq!(fields.len(), COLOUR_ROLES.len(), "{fields:?}");
        assert!(fields.iter().all(|f| COLOUR_ROLES.contains(f)));
        assert!(TRANSLUCENT_ROLES.iter().all(|r| COLOUR_ROLES.contains(r)));
        let mut t = GRAPHITE;
        for role in COLOUR_ROLES {
            assert!(t.set_colour(role, egui::Color32::WHITE) && t.colour(role) == Some(egui::Color32::WHITE), "{role}");
        }
        assert!(!t.set_colour("nope", egui::Color32::WHITE) && t.colour("nope").is_none());
    }

    #[test]
    fn the_folder_lists_good_and_refused_themes_and_ignores_the_rest() {
        let dir = std::env::temp_dir().join(format!("anicraft-themes-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Library::scan(&dir), Library::default(), "no folder: no themes");
        std::fs::create_dir_all(dir.join("sub.json")).expect("folder");
        std::fs::write(dir.join("plum.json"), "\u{feff}{\"anicraft_theme\": 1, \"name\": \"Plum\", \"colors\": {\"accent\": \"#9a3ce0\"}}").expect("write");
        std::fs::write(dir.join("broken.JSON"), "{").expect("write");
        std::fs::write(dir.join("faint.json"), r##"{"anicraft_theme": 1, "colors": {"text": "#55555c"}}"##).expect("write");
        std::fs::write(dir.join("notes.txt"), "not a theme").expect("write");
        std::fs::write(dir.join("huge.json"), vec![b' '; MAX_BYTES as usize + 10]).expect("write");
        std::fs::write(dir.join("binary.json"), [0xff, 0xfe, 0x00]).expect("write");
        let lib = Library::scan(&dir);
        let seen: Vec<(&str, &str, bool)> = lib.entries.iter().map(|e| (e.id.as_str(), e.name(), e.theme.is_ok())).collect();
        assert_eq!(seen, [("binary", "binary", false), ("broken", "broken", false), ("faint", "faint", false), ("huge", "huge", false), ("plum", "Plum", true)]);
        assert_eq!((lib.counts(), lib.left_out), ((1, 4), 0));
        assert_eq!(lib.get("huge").map(|e| e.theme.clone()), Some(Err(Refusal::TooLarge)));
        assert!(matches!(lib.get("faint").map(|e| &e.theme), Some(Err(Refusal::Contrast { first, .. })) if first.pair == "text on panel"));
        assert!(lib.get("nope").is_none());
        // Export picks a name that is free.
        assert_eq!(free_path(&dir, "plum"), Some(dir.join("plum-2.json")));
        assert_eq!(free_path(&dir, "new"), Some(dir.join("new.json")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sample theme in the user guide is one AniCraft uses, with its two-tone icons and its
    /// gradient accent bars, and nothing about it is noted as low in contrast.
    #[test]
    fn the_guides_sample_theme_is_a_good_theme() {
        // (Whatever line endings the checkout gave the chapter.)
        let chapter = include_str!("../docs/guide/preferences-shortcuts-accessibility.md").replace("\r\n", "\n");
        let sample = chapter.split("```json\n").nth(1).and_then(|s| s.split("```").next()).expect("the chapter has a sample theme");
        let t = parse(sample).unwrap_or_else(|e| panic!("{}", e.message()));
        assert_eq!((t.name.as_deref(), t.base, t.notes.len()), (Some("Ember"), Palette::Graphite, 0), "{:?}", t.notes);
        assert!(t.tokens.icon_accent.a() > 0 && t.tokens.accent_end != t.tokens.accent);
    }

    #[test]
    fn hostile_files_never_panic() {
        let deep = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
        for text in ["", "{", "null", "1e999", &deep, r#"{"anicraft_theme": 1e400}"#, r#"{"anicraft_theme": -1}"#, r#"{"anicraft_theme": 1, "colors": {"": ""}}"#, r##"{"anicraft_theme": 1, "colors": {"panel": "#ééé"}}"##, r##"{"anicraft_theme": 1, "colors": {"panel": "#\u0000\u0000\u0000"}}"##] {
            assert!(parse(text).is_err(), "{}", text.chars().take(40).collect::<String>());
        }
        // Every role set to every kind of awkward colour: refused or applied, never a panic.
        for colour in ["#000000", "#ffffff", "#808080", "#00000000", "#ffffff01"] {
            let all: Vec<String> = COLOUR_ROLES.iter().map(|r| format!("\"{r}\": \"{colour}\"")).collect();
            let _ = theme(&format!("\"colors\": {{{}}}", all.join(", ")));
        }
    }
}
