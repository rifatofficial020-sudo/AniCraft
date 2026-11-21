use super::*;
use crate::rich_text::BaselineShift;

fn base() -> CharStyle {
    CharStyle { font: "Arial".into(), size: 12.0, color: Rgba::BLACK, ..CharStyle::default() }
}

fn read(html: &str) -> Parsed {
    parse(html, &base(), &ParagraphStyle::default())
}

/// `(characters, what differs from the base)` per run, for short assertions.
fn runs(p: &Parsed) -> Vec<(u32, String)> {
    let b = base();
    p.styles
        .runs
        .iter()
        .map(|r| {
            let s = &r.style;
            let mut d = Vec::new();
            if s.bold {
                d.push("b".to_string());
            }
            if s.italic {
                d.push("i".to_string());
            }
            if s.underline {
                d.push("u".to_string());
            }
            if s.font != b.font {
                d.push(format!("face={}", s.font));
            }
            if s.size != b.size {
                d.push(format!("size={}", s.size));
            }
            if s.color != b.color {
                d.push(format!("color={:.0},{:.0},{:.0}", s.color.r * 255.0, s.color.g * 255.0, s.color.b * 255.0));
            }
            if s.letter_spacing != 0.0 {
                d.push(format!("ls={}", s.letter_spacing));
            }
            if !s.url.is_empty() {
                d.push(format!("url={}", s.url));
            }
            (r.chars, d.join(" "))
        })
        .collect()
}

fn r(n: u32, s: &str) -> (u32, String) {
    (n, s.to_string())
}

#[test]
fn the_subset_becomes_runs_and_paragraphs() {
    let p = read("Hello <b>bold <i>both</i></b> <u>under</u>");
    assert_eq!(p.text, "Hello bold both under");
    assert_eq!(runs(&p), vec![r(6, ""), r(5, "b"), r(4, "b i"), r(1, ""), r(5, "u")]);
    let p = read(r##"<font face="Georgia" size="20" color="#FF8000" letterspacing="1.5">big</font> <font size="+4">up</font><font size="-2">down</font>"##);
    assert_eq!(p.text, "big updown");
    assert_eq!(runs(&p), vec![r(3, "face=Georgia size=20 color=255,128,0 ls=1.5"), r(1, ""), r(2, "size=16"), r(4, "size=10")]);
    let p = read(r#"<a href="https://example.com/?a=1&amp;b=2">link</a> and <A HREF='page.html'>more</A>"#);
    assert_eq!(runs(&p), vec![r(4, "url=https://example.com/?a=1&b=2"), r(5, ""), r(4, "url=page.html")]);
    // Paragraphs and breaks are lines; each `<p>` has its alignment.
    let p = read(r#"<p align="center">Title</p><p align="right">right</p>plain<br>next<br/>last"#);
    assert_eq!(p.text, "Title\nright\nplain\nnext\nlast");
    let aligns: Vec<TextAlign> = p.styles.paragraphs.iter().map(|q| q.align).collect();
    assert_eq!(aligns, vec![TextAlign::Center, TextAlign::Right, TextAlign::Left, TextAlign::Left, TextAlign::Left]);
    // A paragraph that ends the HTML adds no empty line; one opened mid-line starts a line.
    assert_eq!(read("<p>one</p>").text, "one");
    assert_eq!(read("a<p>b</p>").text, "a\nb");
    assert_eq!(read("<p>a</p><p></p><p>b</p>").text, "a\n\nb");
    assert_eq!(read("<P ALIGN=\"JUSTIFY\">j</P>").styles.paragraphs, vec![ParagraphStyle::default()]);
    // Entities; an unfinished one is text.
    assert_eq!(read("1 &lt; 2 &amp;&amp; 3 &gt; 2 &quot;q&quot; &apos;a&apos; a&nbsp;b &#65;&#x42; &copy; AT&T &").text, "1 < 2 && 3 > 2 \"q\" 'a' a\u{a0}b AB &copy; AT&T &");
    // Empty input is an empty field in the base style.
    let e = read("");
    assert_eq!((e.text.as_str(), e.styles.runs.len(), e.styles.paragraphs.len()), ("", 1, 1));
    assert_eq!(e.styles.runs[0].style, base());
}

#[test]
fn nothing_hostile_is_kept_as_markup_or_as_a_link() {
    // Script and style go with their content; other tags go, their text stays.
    assert_eq!(read("a<script>alert(1)</script>b<STYLE>p{}</STYLE >c<script>never closed").text, "abc");
    assert_eq!(read("<div onclick=\"x()\">div</div><img src=x onerror=alert(1)><span class=k>span</span><textformat leading=\"2\">t</textformat>").text, "divspant");
    assert_eq!(read("x<!-- <b>hidden</b> -->y<!-- open").text, "xy");
    assert_eq!(read("<!DOCTYPE html><?xml version=\"1.0\"?>ok").text, "ok");
    // A `<` that starts no tag is text; so is an unclosed tag's `<`.
    assert_eq!(read("1 < 2 and 3 <4 <b").text, "1 < 2 and 3 <4 <b");
    assert_eq!(read("a <> b").text, "a <> b");
    // A `>` inside a quoted attribute doesn't end the tag; a quote left open ends nothing.
    assert_eq!(read("<font face=\"a>b\">x</font>").styles.runs[0].style.font, "a>b");
    assert_eq!(read("<a href=\"open>text").text, "<a href=\"open>text");
    // Links that could run script are dropped; the text stays, unlinked.
    for bad in ["javascript:alert(1)", " JaVaScRiPt:alert(1)", "java\tscript:alert(1)", "vbscript:x", "data:text/html,<script>1</script>", "&#106;avascript:alert(1)"] {
        let p = read(&format!("<a href=\"{bad}\">click</a>"));
        assert_eq!(p.text, "click", "{bad}");
        assert!(p.styles.runs.iter().all(|r| r.style.url.is_empty()), "{bad}: {:?}", p.styles.runs);
    }
    // Wild values are ignored or clamped.
    let p = read(r##"<font size="1e9" color="red" face="">a</font><font size="99999">b</font><font size="-99999">c</font><font color="#12345">d</font><font letterspacing="NaN">e</font>"##);
    assert_eq!(runs(&p), vec![r(1, ""), r(1, "size=2000"), r(1, "size=1"), r(2, "")]);
    // Control characters and character codes that aren't characters are dropped.
    assert_eq!(read("a\u{0}b\u{7}c&#0;d&#x110000;e&#xD800;f\r\ng\th").text, "abcdef\ng\th");
    // Unbalanced and stray closing tags do no harm; a close ends what was opened inside too.
    let p = read("</b>a<b>b<i>c</b>d</i></font>e");
    assert_eq!((p.text.as_str(), runs(&p)), ("abcde", vec![r(1, ""), r(1, "b"), r(1, "b i"), r(2, "")]));
}

#[test]
fn sizes_are_bounded() {
    // Nesting past the limit stops changing the style, and closes unwind cleanly.
    let deep = format!("{}x{}y", "<b><i>".repeat(500), "</i></b>".repeat(500));
    let p = read(&deep);
    assert_eq!(p.text, "xy");
    assert!(p.styles.runs[0].style.bold && !p.styles.runs.last().unwrap().style.bold);
    // Long input is cut, long text capped, with runs and paragraphs to match.
    let p = read(&"ab<br>".repeat(100_000));
    assert!(p.text.chars().count() <= MAX_FIELD_CHARS);
    assert_eq!(p.styles.runs.iter().map(|r| r.chars as usize).sum::<usize>(), p.text.chars().count());
    assert_eq!(p.styles.paragraphs.len(), p.text.matches('\n').count() + 1);
    let p = read(&"x".repeat(MAX_HTML * 2));
    assert_eq!(p.text.chars().count(), MAX_FIELD_CHARS);
    // A tag with a great many attributes or a huge attribute is read without trouble.
    let p = read(&format!("<font {} size=\"30\">a</font><a href=\"{}\">b</a>", "x=1 ".repeat(1000), "h".repeat(5000)));
    assert_eq!(p.text, "ab");
    assert!(p.styles.runs.iter().all(|r| r.style.url.chars().count() <= crate::rich_text::MAX_URL));
}

#[test]
fn written_html_reads_back_the_same() {
    let p = read(r##"<p align="center">Score: <b>12</b> <font color="#cc0000" size="18.5" face="Georgia">red &amp; <i>big</i></font></p><p align="right"><a href="https://example.com/a?b=&quot;c&quot;"><u>go</u></a> 1 &lt; 2</p>last"##);
    let html = write(&p.text, &p.styles);
    assert_eq!(
        html,
        concat!(
            r##"<p align="center"><font face="Arial" size="12" color="#000000">Score: </font><font face="Arial" size="12" color="#000000"><b>12</b></font><font face="Arial" size="12" color="#000000"> </font>"##,
            r##"<font face="Georgia" size="18.5" color="#cc0000">red &amp; </font><font face="Georgia" size="18.5" color="#cc0000"><i>big</i></font></p>"##,
            r##"<p align="right"><font face="Arial" size="12" color="#000000"><a href="https://example.com/a?b=&quot;c&quot;"><u>go</u></a></font><font face="Arial" size="12" color="#000000"> 1 &lt; 2</font></p>"##,
            r##"<p align="left"><font face="Arial" size="12" color="#000000">last</font></p>"##
        )
    );
    let back = read(&html);
    assert_eq!(back, p, "the same text, runs and paragraphs");
    // What this HTML has no place for is left out: shifts and opacity.
    let mut odd = p.clone();
    odd.styles.runs[0].style.shift = BaselineShift::Superscript;
    odd.styles.runs[0].style.color.a = 0.5;
    odd.styles.runs[0].style.letter_spacing = -0.755;
    let html = write(&odd.text, &odd.styles);
    let again = read(&html);
    assert_eq!(again.styles.runs[0].style.shift, BaselineShift::Normal);
    assert_eq!(again.styles.runs[0].style.color.a, 1.0);
    assert_eq!(again.styles.runs[0].style.letter_spacing, -0.75, "two decimals");
    // Plain text with markup characters survives both ways, empty lines too.
    let plain = Parsed { text: "a <b> & \"c\"\n\n\u{a0}z".into(), styles: RichStyles { runs: vec![TextRun { chars: 15, style: base() }], paragraphs: vec![ParagraphStyle::default(); 3] } };
    assert_eq!(read(&write(&plain.text, &plain.styles)), plain);
    // Runs shorter or longer than the text never panic.
    let short = RichStyles { runs: vec![TextRun { chars: 1, style: base() }], paragraphs: Vec::new() };
    assert_eq!(read(&write("abc\ndef", &short)).text, "abc\ndef");
    assert_eq!(read(&write("abc", &RichStyles::default())).text, "abc");
    let long = RichStyles { runs: vec![TextRun { chars: u32::MAX, style: base() }], paragraphs: Vec::new() };
    assert_eq!(read(&write("abc", &long)).text, "abc");
}

/// Small deterministic generator (xorshift), as the engine's fuzz tests use.
fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

#[test]
fn any_input_reads_safely_and_settles_after_one_round_trip() {
    const PIECES: [&str; 40] = [
        "<", ">", "</", "/>", "&", ";", "#", "\"", "'", "=", " ", "\n", "\r", "\t", "\u{0}", "a", "Zz", "é", "😀", "<b>", "</b>", "<i>", "</i>", "<u>", "</u>", "<font size=\"+3\" color=\"#ff00ff\">", "</font>",
        "<a href=\"javascript:x\">", "<a href=\"http://a.b\">", "</a>", "<p align=\"right\">", "</p>", "<br>", "<script>", "</script>", "<!--", "-->", "&lt;", "&#x41;", "&#99999999;",
    ];
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for round in 0..3000 {
        let n = (rng(&mut seed) % 40) as usize;
        let html: String = (0..n).map(|_| PIECES[(rng(&mut seed) % PIECES.len() as u64) as usize]).collect();
        let p = read(&html);
        let count = p.text.chars().count();
        assert_eq!(p.styles.runs.iter().map(|r| r.chars as usize).sum::<usize>(), count, "round {round}: {html:?}");
        assert_eq!(p.styles.paragraphs.len(), p.text.matches('\n').count() + 1, "round {round}: {html:?}");
        assert!(!p.text.contains('\u{0}') && !p.text.contains('\r'), "{html:?}");
        for r in &p.styles.runs {
            assert!(!r.style.url.to_ascii_lowercase().contains("javascript"), "{html:?}");
            assert!((1.0..=2000.0).contains(&r.style.size));
        }
        // Written and read again it is the same text, and the HTML no longer changes.
        let once = write(&p.text, &p.styles);
        let q = read(&once);
        assert_eq!(q.text, p.text, "round {round}: {html:?} -> {once:?}");
        assert_eq!(write(&q.text, &q.styles), once, "round {round}: {html:?}");
        assert!(!once.contains("<script") && !once.contains("javascript:"), "{once}");
    }
}
