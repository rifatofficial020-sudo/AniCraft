//! `htmlText` (ADR 0107): the small HTML Flash text fields took, read into styled runs and
//! paragraphs (ADR 0085) and written back from them.
//!
//! The subset: `<b>`, `<i>`, `<u>`, `<font face size color letterspacing>`, `<a href>`,
//! `<p align>` and `<br>`, with the entities `&lt;`, `&gt;`, `&amp;`, `&quot;`, `&apos;`,
//! `&nbsp;` and numeric ones. Reading is a sanitiser as much as a parser: nothing in the input
//! is ever kept as markup. A tag outside the subset is dropped and its text kept (as Flash
//! does); `<script>` and `<style>` are dropped with what is inside them; links that could run
//! script are dropped (the text stays); sizes, spacing, lengths and nesting are bounded.
//!
//! Both directions are pure and are mirrored line by line in the HTML export's player
//! (`html/player.js`), which the parity tests hold to the same output.

use crate::rich_text::{CharStyle, ParagraphStyle, TextRun, safe_url};
use crate::{MAX_FIELD_CHARS, Rgba, TextAlign};

/// Most characters of HTML read; the rest is ignored.
pub const MAX_HTML: usize = 65_536;
/// Deepest nesting of style tags that still changes the style; deeper tags are ignored.
pub const MAX_HTML_DEPTH: usize = 64;

/// The styles of a field's text while a movie plays: runs covering the text and one style per
/// paragraph (line).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichStyles {
    pub runs: Vec<TextRun>,
    pub paragraphs: Vec<ParagraphStyle>,
}

/// HTML read into text and its styles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub text: String,
    pub styles: RichStyles,
}

struct Reader {
    text: String,
    count: usize,
    runs: Vec<TextRun>,
    paragraphs: Vec<ParagraphStyle>,
    style: CharStyle,
    para: ParagraphStyle,
    base_para: ParagraphStyle,
    /// The text ends with the line break a `</p>` made (dropped if nothing follows).
    closed_p: bool,
}

impl Reader {
    fn push(&mut self, c: char) {
        if self.count >= MAX_FIELD_CHARS {
            return;
        }
        self.text.push(c);
        self.count += 1;
        match self.runs.last_mut() {
            Some(r) if r.style == self.style => r.chars += 1,
            _ => self.runs.push(TextRun { chars: 1, style: self.style.clone() }),
        }
        if c == '\n' {
            self.paragraphs.push(self.para);
        }
    }

    /// The line break a `</p>` makes: in the style of the text before it (HTML has no style
    /// for it), so a paragraph's last run runs to its end.
    fn end_paragraph(&mut self) {
        if self.count >= MAX_FIELD_CHARS {
            return;
        }
        self.text.push('\n');
        self.count += 1;
        match self.runs.last_mut() {
            Some(r) => r.chars += 1,
            None => self.runs.push(TextRun { chars: 1, style: self.style.clone() }),
        }
        self.paragraphs.push(self.para);
    }

    fn at_line_start(&self) -> bool {
        self.text.is_empty() || self.text.ends_with('\n')
    }

    fn text_char(&mut self, c: char) {
        // Tabs and line breaks are text; other control characters are not.
        if c == '\r' || (c.is_control() && c != '\n' && c != '\t') {
            return;
        }
        self.closed_p = false;
        self.push(c);
    }
}

/// One entity at the start of `s` (after its `&`): the character and how many characters of
/// `s` it took (its `;` included). `None`: not an entity, the `&` is text.
fn entity(s: &[char]) -> Option<(Option<char>, usize)> {
    let end = s.iter().take(12).position(|c| *c == ';')?;
    let name: String = s.iter().take(end).collect();
    let c = match name.as_str() {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let digits = name.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) => u32::from_str_radix(hex, 16).ok()?,
                None if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => digits.parse::<u32>().ok()?,
                _ => return None,
            };
            // A code that isn't a character (or is a control) is dropped, not kept as text.
            char::from_u32(code).filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        }
    };
    Some((c, end + 1))
}

/// Text with its entities read.
fn unescape(s: &[char]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(&c) = s.get(i) {
        i += 1;
        if c == '&'
            && let Some((ch, used)) = entity(s.get(i..).unwrap_or(&[]))
        {
            out.extend(ch);
            i += used;
            continue;
        }
        out.push(c);
    }
    out
}

/// A tag's name (lower case, `/` first when it closes) and attributes (names lower case, values
/// with their entities read), from what is between `<` and `>`.
fn tag(body: &[char]) -> (String, bool, Vec<(String, String)>) {
    let mut i = 0;
    let closing = body.first() == Some(&'/');
    if closing {
        i += 1;
    }
    let mut name = String::new();
    while let Some(c) = body.get(i).filter(|c| c.is_ascii_alphanumeric()) {
        name.push(c.to_ascii_lowercase());
        i += 1;
    }
    let mut attrs: Vec<(String, String)> = Vec::new();
    while attrs.len() < 16 {
        while body.get(i).is_some_and(|c| c.is_whitespace() || *c == '/') {
            i += 1;
        }
        let mut key = String::new();
        while let Some(c) = body.get(i).filter(|c| !c.is_whitespace() && **c != '=' && **c != '/') {
            key.push(c.to_ascii_lowercase());
            i += 1;
        }
        if key.is_empty() {
            break;
        }
        while body.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        let mut value: Vec<char> = Vec::new();
        if body.get(i) == Some(&'=') {
            i += 1;
            while body.get(i).is_some_and(|c| c.is_whitespace()) {
                i += 1;
            }
            match body.get(i).copied().filter(|q| *q == '"' || *q == '\'') {
                Some(q) => {
                    i += 1;
                    while let Some(c) = body.get(i).filter(|c| **c != q) {
                        value.push(*c);
                        i += 1;
                    }
                    i += 1;
                }
                None => {
                    while let Some(c) = body.get(i).filter(|c| !c.is_whitespace()) {
                        value.push(*c);
                        i += 1;
                    }
                }
            }
        }
        attrs.push((key, unescape(&value)));
    }
    (name, closing, attrs)
}

/// Where a tag's `>` is in what follows its `<`: the first one outside a quoted attribute
/// value. `None`: the tag never ends.
fn tag_end(rest: &[char]) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (k, c) in rest.iter().enumerate() {
        match quote {
            Some(q) if *c == q => quote = None,
            Some(_) => {}
            None if *c == '"' || *c == '\'' => quote = Some(*c),
            None if *c == '>' => return Some(k),
            None => {}
        }
    }
    None
}

/// `#RRGGBB` as a colour.
fn hex_color(s: &str) -> Option<Rgba> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    let b = |shift: u32| ((v >> shift) & 255) as f32 / 255.0;
    Some(Rgba::new(b(16), b(8), b(0), 1.0))
}

/// A plain decimal number (`12`, `-1.5`, `+2`): no exponents, at most 12 characters.
fn number(s: &str) -> Option<f32> {
    let s = s.trim();
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || s.len() > 12 || !digits.chars().all(|c| c.is_ascii_digit() || c == '.') || digits.chars().filter(|c| *c == '.').count() > 1 || digits == "." {
        return None;
    }
    s.strip_prefix('+').unwrap_or(s).parse::<f32>().ok().filter(|v| v.is_finite())
}

fn font_tag(style: &mut CharStyle, attrs: &[(String, String)]) {
    for (k, v) in attrs {
        match k.as_str() {
            "face" => {
                let face: String = v.trim().chars().filter(|c| !c.is_control()).take(256).collect();
                if !face.is_empty() {
                    style.font = face;
                }
            }
            // A size, or a step from the size around it (`+2`, `-1`), as Flash reads them.
            "size" => {
                if let Some(n) = number(v) {
                    let relative = v.trim().starts_with(['+', '-']);
                    style.size = (if relative { style.size + n } else { n }).clamp(1.0, 2000.0);
                }
            }
            "color" => {
                if let Some(c) = hex_color(v) {
                    style.color = c;
                }
            }
            "letterspacing" => {
                if let Some(n) = number(v) {
                    style.letter_spacing = n.clamp(-1000.0, 1000.0);
                }
            }
            _ => {}
        }
    }
}

/// Reads `html` (the subset above) into text, runs and paragraphs. `base` is the style text
/// outside any tag gets (the field's own), `para` every paragraph's unless `<p align>` says
/// otherwise. At most [`MAX_HTML`] characters are read and [`MAX_FIELD_CHARS`] kept.
pub fn parse(html: &str, base: &CharStyle, para: &ParagraphStyle) -> Parsed {
    let src: Vec<char> = html.chars().take(MAX_HTML).collect();
    let mut r = Reader { text: String::new(), count: 0, runs: Vec::new(), paragraphs: Vec::new(), style: base.clone(), para: *para, base_para: *para, closed_p: false };
    // Open style tags: the tag and the style it changed.
    let mut open: Vec<(String, CharStyle)> = Vec::new();
    let mut i = 0;
    while let Some(&c) = src.get(i) {
        i += 1;
        if c == '&' {
            if let Some((ch, used)) = entity(src.get(i..).unwrap_or(&[])) {
                i += used;
                if let Some(ch) = ch {
                    r.text_char(ch);
                }
                continue;
            }
            r.text_char('&');
            continue;
        }
        if c != '<' {
            r.text_char(c);
            continue;
        }
        // A comment runs to its `-->` (or the end).
        if src.get(i..i + 3) == Some(&['!', '-', '-']) {
            let rest = src.get(i + 3..).unwrap_or(&[]);
            i = rest.windows(3).position(|w| w == ['-', '-', '>']).map_or(src.len(), |k| i + 3 + k + 3);
            continue;
        }
        // A tag runs to its `>`; a `<` that starts none is text.
        let rest = src.get(i..).unwrap_or(&[]);
        let starts_tag = rest.first().is_some_and(|n| n.is_ascii_alphabetic() || *n == '/' || *n == '!' || *n == '?');
        let Some(len) = tag_end(rest).filter(|_| starts_tag) else {
            r.text_char('<');
            continue;
        };
        let (name, closing, attrs) = tag(rest.get(..len).unwrap_or(&[]));
        i += len + 1;
        match (name.as_str(), closing) {
            // Never text: skipped with everything up to its end tag (or the end).
            ("script" | "style", false) => {
                let mut k = i;
                let mut end = src.len();
                while k < src.len() {
                    if src.get(k) == Some(&'<') && src.get(k + 1) == Some(&'/') {
                        let word: String = src.iter().skip(k + 2).take(name.len()).map(char::to_ascii_lowercase).collect();
                        if word == name {
                            end = src.iter().skip(k).position(|c| *c == '>').map_or(src.len(), |g| k + g + 1);
                            break;
                        }
                    }
                    k += 1;
                }
                i = end;
            }
            ("br", false) => {
                r.closed_p = false;
                r.push('\n');
            }
            ("p", false) => {
                if !r.at_line_start() {
                    r.push('\n');
                }
                r.closed_p = false;
                r.para = r.base_para;
                if let Some((_, v)) = attrs.iter().find(|(k, _)| k == "align") {
                    r.para.align = match v.trim().to_ascii_lowercase().as_str() {
                        "center" => TextAlign::Center,
                        "right" => TextAlign::Right,
                        // Justified text is drawn left-aligned (ADR 0085).
                        "left" | "justify" => TextAlign::Left,
                        _ => r.para.align,
                    };
                }
            }
            ("p", true) => {
                r.end_paragraph();
                r.closed_p = true;
                r.para = r.base_para;
            }
            ("b" | "i" | "u" | "font" | "a", false) => {
                if open.len() >= MAX_HTML_DEPTH {
                    continue;
                }
                open.push((name.clone(), r.style.clone()));
                match name.as_str() {
                    "b" => r.style.bold = true,
                    "i" => r.style.italic = true,
                    "u" => r.style.underline = true,
                    "font" => font_tag(&mut r.style, &attrs),
                    _ => {
                        // A link that could run script is no link; its text stays.
                        let href = attrs.iter().find(|(k, _)| k == "href").map_or("", |(_, v)| v.as_str());
                        r.style.url = safe_url(href).unwrap_or_default();
                    }
                }
            }
            ("b" | "i" | "u" | "font" | "a", true) => {
                // Closes the nearest one open (and what was opened inside it).
                if let Some(k) = open.iter().rposition(|(n, _)| *n == name) {
                    if let Some((_, before)) = open.get(k) {
                        r.style = before.clone();
                    }
                    open.truncate(k);
                }
            }
            // Any other tag is dropped; what is inside it is text.
            _ => {}
        }
    }
    if r.closed_p && r.text.ends_with('\n') {
        r.text.pop();
        if let Some(last) = r.runs.last_mut() {
            last.chars -= 1;
            if last.chars == 0 {
                r.runs.pop();
            }
        }
        // The line the `</p>` ended keeps its own alignment.
        r.para = r.paragraphs.pop().unwrap_or(r.base_para);
    }
    r.paragraphs.push(r.para);
    if r.runs.is_empty() {
        r.runs.push(TextRun { chars: 0, style: base.clone() });
    }
    Parsed { text: r.text, styles: RichStyles { runs: r.runs, paragraphs: r.paragraphs } }
}

/// A number as HTML attributes hold it: two decimals at most, no trailing zeros.
fn attr_number(v: f32) -> String {
    let v = if v.is_finite() { f64::from(v) } else { 0.0 };
    format!("{}", (v * 100.0 + 0.5).floor() / 100.0)
}

fn escape(out: &mut String, s: &str, attr: bool) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' if attr => out.push_str("&quot;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            c => out.push(c),
        }
    }
}

/// `text` in `styles` as HTML in the subset: one `<p align>` per line, a `<font>` per run with
/// its face, size, colour and letter spacing, and `<a>`, `<b>`, `<i>` and `<u>` inside it.
/// Baseline shifts, opacity, indents, margins and line spacing have no place in this HTML and
/// are left out. [`parse`] reads it back to the same text, runs and paragraphs.
pub fn write(text: &str, styles: &RichStyles) -> String {
    let mut out = String::with_capacity(text.len() * 2 + 64);
    let chars: Vec<char> = text.chars().collect();
    let last_style = styles.runs.last().map(|r| r.style.clone()).unwrap_or_default();
    // Each character's run.
    let mut run_of = styles.runs.iter().enumerate().flat_map(|(k, r)| std::iter::repeat_n(k, r.chars as usize));
    let mut line: Vec<(usize, char)> = Vec::new();
    let mut para = 0usize;
    let flush = |line: &mut Vec<(usize, char)>, para: usize, out: &mut String| {
        let align = match styles.paragraphs.get(para).or(styles.paragraphs.last()).map_or(TextAlign::Left, |p| p.align) {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
        };
        out.push_str("<p align=\"");
        out.push_str(align);
        out.push_str("\">");
        let mut i = 0;
        while let Some(&(k, _)) = line.get(i) {
            let st = styles.runs.get(k).map_or(&last_style, |r| &r.style);
            let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            out.push_str("<font face=\"");
            escape(out, &st.font, true);
            out.push_str(&format!("\" size=\"{}\" color=\"#{:02x}{:02x}{:02x}\"", attr_number(st.size), b(st.color.r), b(st.color.g), b(st.color.b)));
            if st.letter_spacing != 0.0 {
                out.push_str(&format!(" letterspacing=\"{}\"", attr_number(st.letter_spacing)));
            }
            out.push('>');
            if !st.url.is_empty() {
                out.push_str("<a href=\"");
                escape(out, &st.url, true);
                out.push_str("\">");
            }
            for (on, t) in [(st.bold, "<b>"), (st.italic, "<i>"), (st.underline, "<u>")] {
                if on {
                    out.push_str(t);
                }
            }
            let mut piece = String::new();
            while let Some(&(_, c)) = line.get(i).filter(|(k2, _)| *k2 == k) {
                piece.push(c);
                i += 1;
            }
            escape(out, &piece, false);
            for (on, t) in [(st.underline, "</u>"), (st.italic, "</i>"), (st.bold, "</b>")] {
                if on {
                    out.push_str(t);
                }
            }
            if !st.url.is_empty() {
                out.push_str("</a>");
            }
            out.push_str("</font>");
        }
        out.push_str("</p>");
        line.clear();
    };
    for c in chars {
        let k = run_of.next().unwrap_or(styles.runs.len().saturating_sub(1));
        if c == '\n' {
            flush(&mut line, para, &mut out);
            para += 1;
        } else {
            line.push((k, c));
        }
    }
    flush(&mut line, para, &mut out);
    out
}

#[cfg(test)]
#[path = "html_text_tests.rs"]
mod tests;
