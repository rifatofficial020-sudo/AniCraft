//! Fonts made in memory for tests, so layout that depends on which letters a face has (mixed
//! directions, fallback faces: ADR 0130) is tested the same on every computer. Our own work:
//! the tables are written here from the OpenType specification ("Font file", `head`, `hhea`,
//! `maxp`, `hmtx`, `cmap` format 12, `loca`, `glyf`, `name`, `post`), and every letter is a box.

fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend(v.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend(v.to_be_bytes());
}

/// The em square, and how tall a letter's box and the "notdef" box are (font units).
pub(crate) const EM: u16 = 1000;
pub(crate) const LETTER_HEIGHT: i16 = 700;
pub(crate) const NOTDEF_HEIGHT: i16 = 300;

/// A TrueType font named `family` with a glyph for each character of `chars`, every one
/// `advance` font units wide: a box [`LETTER_HEIGHT`] tall standing on the baseline (white
/// space: nothing). Its "notdef" glyph is a box [`NOTDEF_HEIGHT`] tall. No kerning, ligatures
/// or mark positioning.
pub(crate) fn font(family: &str, chars: &str, advance: u16) -> Vec<u8> {
    let mut cps: Vec<char> = chars.chars().collect();
    cps.sort_unstable();
    cps.dedup();
    let n = cps.len() as u16 + 1;
    let right = advance.saturating_sub(50).max(60) as i16;

    let (mut glyf, mut loca) = (Vec::new(), vec![0u32]);
    for g in 0..n {
        let c = g.checked_sub(1).and_then(|i| cps.get(i as usize)).copied();
        if !c.is_some_and(char::is_whitespace) {
            let top = if c.is_some() { LETTER_HEIGHT } else { NOTDEF_HEIGHT };
            for v in [1, 50, 0, right, top] {
                glyf.extend(i16::to_be_bytes(v));
            }
            be16(&mut glyf, 3); // the contour's last point
            be16(&mut glyf, 0); // no instructions
            glyf.extend([1u8; 4]); // four on-curve points, coordinates as words
            for v in [50, 0, right - 50, 0, 0, top, 0, -top] {
                glyf.extend(i16::to_be_bytes(v));
            }
            while glyf.len() % 4 != 0 {
                glyf.push(0);
            }
        }
        loca.push(glyf.len() as u32);
    }
    let loca: Vec<u8> = loca.iter().flat_map(|o| o.to_be_bytes()).collect();

    let mut head = Vec::new();
    be32(&mut head, 0x0001_0000);
    be32(&mut head, 0x0001_0000);
    be32(&mut head, 0);
    be32(&mut head, 0x5F0F_3CF5);
    be16(&mut head, 3);
    be16(&mut head, EM);
    head.extend([0u8; 16]); // created, modified
    for v in [0, 0, right, LETTER_HEIGHT] {
        head.extend(i16::to_be_bytes(v));
    }
    be16(&mut head, 0);
    be16(&mut head, 8);
    for v in [2i16, 1, 0] {
        head.extend(v.to_be_bytes()); // direction hint, long `loca` offsets, glyph format
    }

    let mut hhea = Vec::new();
    be32(&mut hhea, 0x0001_0000);
    for v in [800i16, -200, 0] {
        hhea.extend(v.to_be_bytes());
    }
    be16(&mut hhea, advance);
    for v in [0i16, 0, right, 1, 0, 0, 0, 0, 0, 0, 0] {
        hhea.extend(v.to_be_bytes());
    }
    be16(&mut hhea, n);

    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0001_0000);
    for v in [n, 4, 1, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0] {
        be16(&mut maxp, v);
    }

    let mut hmtx = Vec::new();
    for _ in 0..n {
        be16(&mut hmtx, advance);
        be16(&mut hmtx, 50);
    }

    let mut cmap = Vec::new();
    for v in [0u16, 1, 3, 10] {
        be16(&mut cmap, v);
    }
    be32(&mut cmap, 12);
    be16(&mut cmap, 12);
    be16(&mut cmap, 0);
    be32(&mut cmap, 16 + 12 * cps.len() as u32);
    be32(&mut cmap, 0);
    be32(&mut cmap, cps.len() as u32);
    for (i, c) in cps.iter().enumerate() {
        for v in [*c as u32, *c as u32, i as u32 + 1] {
            be32(&mut cmap, v);
        }
    }

    let names = [(1u16, family.to_string()), (2, "Regular".to_string()), (4, family.to_string()), (6, family.replace(' ', ""))];
    let (mut name, mut strings) = (Vec::new(), Vec::new());
    be16(&mut name, 0);
    be16(&mut name, names.len() as u16);
    be16(&mut name, 6 + 12 * names.len() as u16);
    for (id, s) in &names {
        let utf16: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
        for v in [3, 1, 0x0409, *id, utf16.len() as u16, strings.len() as u16] {
            be16(&mut name, v);
        }
        strings.extend(utf16);
    }
    name.extend(strings);

    let mut post = Vec::new();
    be32(&mut post, 0x0003_0000);
    be32(&mut post, 0);
    post.extend(i16::to_be_bytes(-100));
    post.extend(i16::to_be_bytes(50));
    post.extend([0u8; 20]);

    let tables: [(&[u8; 4], Vec<u8>); 9] = [(b"cmap", cmap), (b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp), (b"name", name), (b"post", post)];
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    for v in [9u16, 128, 3, 16] {
        be16(&mut out, v);
    }
    let mut offset = 12 + 16 * tables.len() as u32;
    let mut body = Vec::new();
    for (tag, data) in &tables {
        let mut padded = data.clone();
        while padded.len() % 4 != 0 {
            padded.push(0);
        }
        let sum = padded.as_chunks::<4>().0.iter().fold(0u32, |a, w| a.wrapping_add(u32::from_be_bytes(*w)));
        out.extend(*tag);
        be32(&mut out, sum);
        be32(&mut out, offset);
        be32(&mut out, data.len() as u32);
        offset += padded.len() as u32;
        body.extend(padded);
    }
    out.extend(body);
    out
}
