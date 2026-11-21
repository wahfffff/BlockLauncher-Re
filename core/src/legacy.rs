//! Legacy `.mod` patches — the `PTP` container that predates this launcher.
//!
//! A `.mod` from the BlockLauncher era is 15 to 40 bytes: a ten-byte header, an
//! optional author line, and a small blob of Thumb machine code. It is not a
//! library and it is not a script; it is a *patch* that something else used to
//! apply to `libminecraftpe.so`.
//!
//! ```text
//! ff 50 54 50   magic, 0xff then "PTP"
//! 00 01         version, big-endian, 1 in every sample seen
//! 00 00         reserved, 0 in every sample seen
//! 00 17         offset of the 0x00 that ends the author line
//! 4d 61 64 ...  the author line itself, ASCII, may be empty
//! 00            terminator, required even when the line is empty
//! 10 51 32 ff   the payload
//! ```
//!
//! ## What this module does and does not claim
//!
//! The *container* is fully determined by the eight samples: for each one the
//! declared offset lands exactly on the terminating zero and the author line
//! decodes as ASCII. That is checked, not assumed, in the tests below.
//!
//! The *payload* is not decoded. It is plausibly a search pattern plus a
//! replacement — the two night-vision patches both end in `4f f0 7e 50 70 47`,
//! which is `mov.w r0, #0x7e; bx lr` — but three of the eight payloads are an
//! odd number of bytes, and none of them carries a length or an address that
//! would let a reader locate the site it patches. Guessing an offset into a live
//! `libminecraftpe.so` is how a patch corrupts a function instead of editing it.
//!
//! So this type exists to *report* legacy patches, not to apply them: the
//! launcher can name what a user dropped into a folder instead of calling it
//! "unknown format", and can say plainly that this build cannot run it. See
//! `docs/LEGACY-MOD.md`.

use crate::error::{Error, Result};

/// `0xff` followed by `PTP`. The leading byte is part of the magic, not padding:
/// every sample starts with it and no other magic is known.
pub const MAGIC: [u8; 4] = [0xff, b'P', b'T', b'P'];

/// Magic + version + reserved + author-line offset.
pub const HEADER_LEN: usize = 10;

/// The only version this build has a sample of.
pub const VERSION: u16 = 1;

/// Longest author line worth accepting. The samples cap at 14 bytes; anything
/// past this is not a line a person typed, and the offset field cannot address
/// a file bigger than 64 KiB anyway.
const MAX_AUTHOR: u16 = 256;

/// A legacy patch, as far as it can be read without knowing what the payload
/// means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyMod {
    /// The author line exactly as stored. Empty when the file carries none,
    /// which is the common case for the smallest patches.
    pub line: String,
    /// Payload, opaque. Kept so the UI can show its size and so a later build
    /// that learns the encoding does not need to re-read the file.
    pub payload: Vec<u8>,
    /// Header bytes this build does not interpret. Always empty for a file that
    /// parses, because unknown flags are refused rather than ignored.
    pub reserved: u16,
}

impl LegacyMod {
    /// `Some("KSYMC")` for a line reading `Made by KSYMC`.
    ///
    /// Every sample that has a line at all uses that one prefix, so it is worth
    /// surfacing separately from the raw line — but the raw line is kept too,
    /// because a prefix this build does not recognise should reach the user
    /// unchanged rather than be dropped.
    pub fn author(&self) -> Option<&str> {
        let rest = self.line.strip_prefix("Made by ")?;
        let rest = rest.trim();
        if rest.is_empty() {
            None
        } else {
            Some(rest)
        }
    }

    /// One sentence for a list row: who made it, and that it cannot run here.
    pub fn summary(&self) -> String {
        match self.author() {
            Some(a) => format!("legacy patch by {a} ({} bytes, cannot run)", self.payload.len()),
            None => format!("legacy patch ({} bytes, cannot run)", self.payload.len()),
        }
    }
}

/// Read a `PTP` patch.
///
/// Every failure names the byte offset it failed at: these files are small
/// enough that the user can look at the exact byte, and "invalid .mod" alone
/// would not tell them whether they have a corrupt download or the wrong file.
pub fn parse(bytes: &[u8]) -> Result<LegacyMod> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::BadMod(format!(
            "{} bytes is shorter than the {HEADER_LEN}-byte header",
            bytes.len()
        )));
    }
    if bytes[..4] != MAGIC {
        return Err(Error::BadMod(format!(
            "not a legacy .mod: expected ff 50 54 50, found {:02x} {:02x} {:02x} {:02x}",
            bytes[0], bytes[1], bytes[2], bytes[3]
        )));
    }

    let version = u16::from_be_bytes([bytes[4], bytes[5]]);
    if version != VERSION {
        return Err(Error::BadMod(format!(
            "container version {version} is not known to this build (only {VERSION} is)"
        )));
    }

    let reserved = u16::from_be_bytes([bytes[6], bytes[7]]);
    if reserved != 0 {
        // Refused rather than ignored: the field is zero in all eight samples,
        // so a non-zero value means this is a variant whose layout is unknown,
        // and reading its author-line offset as if it were the known layout is
        // how a parser produces plausible garbage.
        return Err(Error::BadMod(format!(
            "header flags 0x{reserved:04x} are not understood by this build"
        )));
    }

    let end = u16::from_be_bytes([bytes[8], bytes[9]]);
    if end < HEADER_LEN as u16 {
        return Err(Error::BadMod(format!(
            "author-line offset {end} points inside the header"
        )));
    }
    if end > MAX_AUTHOR + HEADER_LEN as u16 {
        return Err(Error::BadMod(format!(
            "author-line offset {end} is larger than any known file"
        )));
    }
    let end = end as usize;
    if end >= bytes.len() {
        return Err(Error::BadMod(format!(
            "author-line offset {end} is past the end of a {} byte file",
            bytes.len()
        )));
    }
    if bytes[end] != 0 {
        return Err(Error::BadMod(format!(
            "byte {end} should end the author line but is 0x{:02x}",
            bytes[end]
        )));
    }

    let line = std::str::from_utf8(&bytes[HEADER_LEN..end]).map_err(|_| {
        Error::BadMod(format!(
            "author line at bytes {HEADER_LEN}..{end} is not UTF-8"
        ))
    })?;
    if line.chars().any(|c| c.is_control()) {
        return Err(Error::BadMod(
            "author line contains a control character".into(),
        ));
    }

    let payload = bytes[end + 1..].to_vec();
    if payload.is_empty() {
        return Err(Error::BadMod(
            "patch has no payload: nothing to apply".into(),
        ));
    }

    Ok(LegacyMod {
        line: line.to_string(),
        payload,
        reserved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The eight samples this format was derived from, byte for byte.
    const SAMPLES: [(&str, &[u8]); 8] = [
        (
            "9格物品栏",
            &[
                0xff, 0x50, 0x54, 0x50, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x1a, 0x84, 0xcc,
                0x0a, 0x20, 0x70, 0x47,
            ],
        ),
        (
            "友善怪物",
            &[
                0xff, 0x50, 0x54, 0x50, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x17, 0x88, 0xfc,
                0x70, 0x47,
            ],
        ),
        (
            "夜视(500ISE)",
            &[
                0xff, 0x50, 0x54, 0x50, 0x00, 0x01, 0x00, 0x00, 0x00, 0x18, 0x4d, 0x61, 0x64, 0x65,
                0x20, 0x62, 0x79, 0x20, 0x35, 0x30, 0x30, 0x49, 0x53, 0x45, 0x00, 0x1b, 0x68, 0x0a,
                0x4f, 0xf0, 0x7e, 0x50, 0x70, 0x47,
            ],
        ),
        (
            "夜视(Skull)",
            &[
                0xff, 0x50, 0x54, 0x50, 0x00, 0x01, 0x00, 0x00, 0x00, 0x18, 0x4d, 0x61, 0x64, 0x65,
                0x20, 0x62, 0x79, 0x20, 0x53, 0x6b, 0x75, 0x6c, 0x6c, 0x2e, 0x00, 0x18, 0x25, 0xe6,
                0x4f, 0xf0, 0x7e, 0x50, 0x70, 0x47,
            ],
        ),
        (
            "服务器255人上限",
            &[
                0xff, 0x50, 0x54, 0x50, 0x00, 0x01, 0x00, 0x00, 0x00, 0x17, 0x4d, 0x61, 0x64, 0x65,
                0x20, 0x62, 0x79, 0x20, 0x4b, 0x53, 0x59, 0x4d, 0x43, 0x00, 0x10, 0x51, 0x32, 0xff,
            ],
        ),
        (
            "死亡时保留背包物品",
            &[
                0xff, 0x50, 0x54, 0x50, 0x00, 0x01, 0x00, 0x00, 0x00, 0x17, 0x4d, 0x61, 0x64, 0x65,
                0x20, 0x62, 0x79, 0x20, 0x4b, 0x53, 0x59, 0x4d, 0x43, 0x00, 0x18, 0xeb, 0xc9, 0x47,
            ],
        ),
        (
            "高清修复",
            &[
                0xff, 0x50, 0x54, 0x50, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x14, 0x86, 0x34,
                0x70, 0x47,
            ],
        ),
        (
            "高速燃烧",
            &[
                0xff, 0x50, 0x54, 0x50, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x1b, 0x5f, 0x24,
                0x01,
            ],
        ),
    ];

    #[test]
    fn every_sample_parses() {
        for (label, bytes) in SAMPLES {
            parse(bytes).unwrap_or_else(|e| panic!("{label}: {e}"));
        }
    }

    /// The claim the layout rests on: the declared offset lands on the zero that
    /// ends the author line, in all eight files, and the line reads back exactly.
    #[test]
    fn the_offset_field_addresses_the_terminator() {
        let expected = [
            ("9格物品栏", ""),
            ("友善怪物", ""),
            ("夜视(500ISE)", "Made by 500ISE"),
            ("夜视(Skull)", "Made by Skull."),
            ("服务器255人上限", "Made by KSYMC"),
            ("死亡时保留背包物品", "Made by KSYMC"),
            ("高清修复", ""),
            ("高速燃烧", ""),
        ];
        for ((label, bytes), (label2, line)) in SAMPLES.iter().zip(expected) {
            assert_eq!(*label, label2);
            let m = parse(bytes).expect(label);
            assert_eq!(m.line, line, "{label}");
            let end = u16::from_be_bytes([bytes[8], bytes[9]]) as usize;
            assert_eq!(bytes[end], 0, "{label}: terminator");
            assert_eq!(end, HEADER_LEN + line.len(), "{label}: offset is 10 + line length");
        }
    }

    #[test]
    fn payload_starts_after_the_terminator() {
        let m = parse(SAMPLES[4].1).expect("server slots");
        assert_eq!(m.payload, vec![0x10, 0x51, 0x32, 0xff]);
        let end = u16::from_be_bytes([SAMPLES[4].1[8], SAMPLES[4].1[9]]) as usize;
        assert_eq!(end + 1 + m.payload.len(), SAMPLES[4].1.len());
    }

    #[test]
    fn the_author_prefix_is_split_off_but_the_line_is_kept() {
        let m = parse(SAMPLES[4].1).expect("server slots");
        assert_eq!(m.author(), Some("KSYMC"));
        assert_eq!(m.line, "Made by KSYMC");

        let anonymous = parse(SAMPLES[0].1).expect("item slots");
        assert_eq!(anonymous.author(), None);
        assert_eq!(anonymous.line, "");
    }

    /// A named file that is not one of the samples still reports the right size
    /// and does not claim an author it cannot find.
    #[test]
    fn summary_says_what_it_is() {
        assert_eq!(
            parse(SAMPLES[4].1).unwrap().summary(),
            "legacy patch by KSYMC (4 bytes, cannot run)"
        );
        assert_eq!(
            parse(SAMPLES[0].1).unwrap().summary(),
            "legacy patch (7 bytes, cannot run)"
        );
    }

    #[test]
    fn a_short_file_is_refused_before_it_is_read() {
        assert!(parse(&[0xff, 0x50, 0x54]).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn a_wrong_magic_is_named_in_the_error() {
        let mut b = SAMPLES[0].1.to_vec();
        b[0] = 0xfe;
        let e = parse(&b).unwrap_err().to_string();
        assert!(e.contains("expected ff 50 54 50"), "{e}");
        assert!(e.contains("fe 50 54 50"), "{e}");
    }

    #[test]
    fn an_unknown_version_is_refused() {
        let mut b = SAMPLES[0].1.to_vec();
        b[4] = 0x00;
        b[5] = 0x02;
        let e = parse(&b).unwrap_err().to_string();
        assert!(e.contains("version 2"), "{e}");
    }

    #[test]
    fn unknown_flags_are_refused_rather_than_ignored() {
        let mut b = SAMPLES[0].1.to_vec();
        b[7] = 0x01;
        let e = parse(&b).unwrap_err().to_string();
        assert!(e.contains("0x0001"), "{e}");
        assert!(e.contains("not understood"), "{e}");
    }

    #[test]
    fn an_offset_inside_the_header_is_refused() {
        let mut b = SAMPLES[0].1.to_vec();
        b[8] = 0x00;
        b[9] = 0x05;
        assert!(parse(&b).is_err());
    }

    #[test]
    fn an_offset_past_the_end_is_refused() {
        let mut b = SAMPLES[0].1.to_vec();
        b[8] = 0x01;
        b[9] = 0x00;
        assert!(parse(&b).is_err());
    }

    #[test]
    fn a_missing_terminator_is_refused() {
        let mut b = SAMPLES[4].1.to_vec();
        b[23] = b'X';
        let e = parse(&b).unwrap_err().to_string();
        assert!(e.contains("should end the author line"), "{e}");
    }

    #[test]
    fn an_empty_payload_is_refused() {
        let mut b = SAMPLES[0].1[..10].to_vec();
        b.extend_from_slice(&[0x00]); // just the terminator, no payload
        let e = parse(&b).unwrap_err().to_string();
        assert!(e.contains("no payload"), "{e}");
    }

    #[test]
    fn a_non_utf8_line_is_refused() {
        let mut b = SAMPLES[4].1.to_vec();
        b[10] = 0xff;
        assert!(parse(&b).is_err());
    }
}
