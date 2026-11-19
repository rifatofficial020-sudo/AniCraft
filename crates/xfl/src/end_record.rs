//! `.fla` archives whose end record overstates the size of the central directory (#356).
//!
//! Flash CS5 to Animate write the end-of-central-directory record with a directory size larger
//! than the directory is whenever `mimetype` isn't the archive's first entry: larger by exactly
//! that entry's record (46 bytes and its 8-byte name). Animate and `unzip` read such files; the
//! shared reader (`craft-xfl`) holds the record to its word and refuses them as damaged ("the
//! central directory is outside the archive"), which was most real `.fla` files.
//!
//! [`with_measured_directory`] corrects the size in a copy when, and only when, the directory
//! the record points at holds exactly the entries it counts and ends exactly where the record
//! begins. The shared reader then reads the copy with every check it has (bounds, overlaps,
//! checksums, limits), so nothing is trusted here beyond those four bytes. Written from the
//! published format (PKWARE APPNOTE 6.3, sections 4.3.12 and 4.3.16).
//!
//! This belongs in the shared reader (storytold/craft-libs#1986 proposes it there); it goes
//! from here when the pinned `craft-xfl` has it.

/// "PK\x05\x06": the end-of-central-directory record.
const END: u32 = 0x0605_4b50;
/// "PK\x01\x02": a central directory entry.
const CENTRAL: u32 = 0x0201_4b50;
/// The end record without its comment, and the longest comment it can carry.
const END_LEN: usize = 22;
const MAX_COMMENT: usize = 65_535;
/// A central directory entry without its name, extra field and comment.
const CENTRAL_LEN: usize = 46;
/// End records looked into (a comment can hold thousands of look-alikes, each naming a
/// directory of thousands of entries).
const MAX_CANDIDATES: usize = 8;

fn u16_at(data: &[u8], at: usize) -> Option<usize> {
    let b = data.get(at..at.checked_add(2)?)?;
    Some(usize::from(u16::from_le_bytes([*b.first()?, *b.get(1)?])))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*b.first()?, *b.get(1)?, *b.get(2)?, *b.get(3)?]))
}

/// The directory's real size, if the end record at `end` overstates it and everything else
/// about the record holds: one file, not ZIP64, and its `count` entries fill the bytes from
/// the directory's offset to the record exactly.
fn measured(data: &[u8], end: usize) -> Option<u32> {
    let (disk, dir_disk, on_disk, count) = (u16_at(data, end + 4)?, u16_at(data, end + 6)?, u16_at(data, end + 8)?, u16_at(data, end + 10)?);
    let (size, offset) = (u32_at(data, end + 12)?, u32_at(data, end + 16)?);
    if disk != 0 || dir_disk != 0 || on_disk != count || count == 0 || count == 0xffff || size == u32::MAX || offset == u32::MAX {
        return None;
    }
    let start = usize::try_from(offset).ok()?;
    let real = u32::try_from(end.checked_sub(start)?).ok()?;
    if real >= size {
        return None;
    }
    let mut at = start;
    for _ in 0..count {
        if u32_at(data, at)? != CENTRAL {
            return None;
        }
        let (name, extra, comment) = (u16_at(data, at + 28)?, u16_at(data, at + 30)?, u16_at(data, at + 32)?);
        at = at.checked_add(CENTRAL_LEN + name + extra + comment)?;
        if at > end {
            return None;
        }
    }
    (at == end).then_some(real)
}

/// `data` with its end record's directory size corrected to what the directory measures, or
/// `None` when the archive isn't of that kind (then the reader's own error stands). The
/// earliest fitting record wins, so a copy of one in the archive's comment can't stand in for it.
pub(crate) fn with_measured_directory(data: &[u8]) -> Option<Vec<u8>> {
    let last = data.len().checked_sub(END_LEN)?;
    let first = last.saturating_sub(MAX_COMMENT);
    let mut looked = 0;
    for end in first..=last {
        if u32_at(data, end) != Some(END) || !u16_at(data, end + 20).is_some_and(|comment| end + END_LEN + comment <= data.len()) {
            continue;
        }
        if let Some(real) = measured(data, end) {
            let mut out = data.to_vec();
            out.get_mut(end + 12..end + 16)?.copy_from_slice(&real.to_le_bytes());
            return Some(out);
        }
        looked += 1;
        if looked >= MAX_CANDIDATES {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::pack::{Raw, write};
    use anicraft_text::Fonts;

    const DOC: &str = r#"<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="320" height="240" frameRate="24"><timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Layer 1"><frames><DOMFrame index="0" duration="12"><elements/></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timelines></DOMDocument>"#;

    /// An archive as Flash writes one: the document first, `mimetype` last.
    fn fla() -> Vec<u8> {
        write(&[Raw::new("DOMDocument.xml", DOC.as_bytes(), true), Raw::new("demo.xfl", b"PROXY-CS5", false), Raw::new("mimetype", b"application/vnd.adobe.xfl", false)], false)
    }

    /// The same with the end record's directory size `by` bytes off.
    fn overstated(by: i64) -> Vec<u8> {
        let mut z = fla();
        let end = z.len() - END_LEN;
        let size = i64::from(u32_at(&z, end + 12).unwrap()) + by;
        z[end + 12..end + 16].copy_from_slice(&u32::try_from(size).unwrap().to_le_bytes());
        z
    }

    fn open(z: &[u8]) -> Result<anicraft_doc::Document, String> {
        crate::import_fla_with_fonts(z, &Fonts::from_faces(Vec::new())).map(|(d, _)| d).map_err(|e| e.to_string())
    }

    /// #356, #385: Flash's end record says the directory is 54 bytes (the `mimetype` entry's
    /// record) longer than it is. The file opens, exactly as the same file with the right size.
    #[test]
    fn a_fla_whose_end_record_overstates_the_directory_opens() {
        let good = open(&fla()).unwrap();
        for by in [54, 1, 46, 60_000] {
            let z = overstated(by);
            assert!(crate::zip::Archive::open(&z).is_err(), "+{by}: the shared reader refuses it (drop this module once it doesn't)");
            let doc = open(&z).unwrap_or_else(|e| panic!("+{by}: {e}"));
            assert!(doc == good, "+{by}: the same document");
            assert_eq!((doc.stage.width, doc.stage.height), (320, 240));
        }
        // With a comment after the end record too.
        let mut z = overstated(54);
        let end = z.len() - END_LEN;
        z[end + 20..end + 22].copy_from_slice(&5u16.to_le_bytes());
        z.extend_from_slice(b"hello");
        assert!(open(&z).unwrap() == good);
    }

    /// Only that one thing is forgiven: a directory that doesn't hold exactly the entries
    /// counted, or doesn't end at the record, is still refused with the reader's own words.
    #[test]
    fn real_damage_is_still_refused() {
        let outside = "the archive is damaged: the central directory is outside the archive";
        // A byte lost from the directory: its entries no longer end at the record.
        let mut z = overstated(54);
        let end = z.len() - END_LEN;
        z.remove(end - 1);
        assert_eq!(open(&z).unwrap_err(), outside);
        // One entry more counted than there is.
        let mut z = overstated(54);
        let end = z.len() - END_LEN;
        for at in [end + 8, end + 10] {
            z[at..at + 2].copy_from_slice(&4u16.to_le_bytes());
        }
        assert_eq!(open(&z).unwrap_err(), outside);
        // A directory offset inside the directory.
        let mut z = overstated(54);
        let end = z.len() - END_LEN;
        let offset = u32_at(&z, end + 16).unwrap() + 3;
        z[end + 16..end + 20].copy_from_slice(&offset.to_le_bytes());
        assert!(open(&z).is_err());
        // An understated size is not this (and nothing here touches it).
        assert!(with_measured_directory(&overstated(-1)).is_none());
        // A healthy archive is left alone, as are files that aren't archives.
        assert!(with_measured_directory(&fla()).is_none());
        assert!(with_measured_directory(b"").is_none());
        assert!(with_measured_directory(b"PK\x05\x06").is_none());
        assert!(with_measured_directory(&[0x50, 0x4b, 0x05, 0x06].repeat(40_000)).is_none());
    }

    /// Every cut and every single flipped byte of such a file: an error or a document, never a
    /// panic, and the correction never reads outside the file.
    #[test]
    fn damaged_overstated_archives_never_panic() {
        let z = overstated(54);
        for cut in 0..z.len() {
            let _ = open(&z[..cut]);
        }
        let dir = u32_at(&z, z.len() - END_LEN + 16).unwrap() as usize;
        for at in dir..z.len() {
            for bit in [0x01, 0x80, 0xff] {
                let mut d = z.clone();
                d[at] ^= bit;
                let _ = open(&d);
            }
        }
    }
}
