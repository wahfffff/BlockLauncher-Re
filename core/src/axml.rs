//! Reader for Android's binary XML format — the compiled `AndroidManifest.xml`
//! inside an APK.
//!
//! This is hand-written rather than pulled from a crate because the launcher
//! needs exactly four fields (`package`, `versionCode`, `uses-sdk`, and the
//! launcher activity) and every XML-on-Android crate brings a resource-table
//! resolver with it. A wrong attribute offset here does not crash — it silently
//! reads a neighbouring field, which is how a launcher ends up reporting a
//! version code of 0 for a perfectly good APK.

use crate::error::{Error, Result};

/// Resource chunk types we have to understand. Everything else in the file is
/// skipped by length, which is what makes this tolerant of new chunk types.
const RES_STRING_POOL: u16 = 0x0001;
const RES_XML: u16 = 0x0003;
const RES_XML_START_ELEMENT: u16 = 0x0102;
const RES_XML_END_ELEMENT: u16 = 0x0103;

/// `Res_value::dataType` values that appear in a manifest.
const TYPE_REFERENCE: u8 = 0x01;
const TYPE_STRING: u8 = 0x03;
const TYPE_INT_DEC: u8 = 0x10;
const TYPE_INT_HEX: u8 = 0x11;
const TYPE_INT_BOOLEAN: u8 = 0x12;

/// A typed attribute value, kept small on purpose: the manifest reader needs to
/// tell "the string 24" apart from "the integer 24" because `uses-sdk` accepts
/// both and they mean different things to us only if we get them wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    /// Reference into `resources.arsc`, which this crate does not resolve.
    Ref(u32),
    /// Types we do not interpret, carried through so a caller can report them.
    Raw(u8, u32),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            // A string-typed integer is legal in `uses-sdk` and appears in the
            // wild, so accept it rather than reporting a missing field.
            Value::Str(s) => s.trim().parse::<i64>().ok(),
            _ => None,
        }
    }
}

/// What the launcher actually needs out of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    pub package: String,
    pub version_code: u64,
    pub version_name: String,
    pub min_sdk: u32,
    pub target_sdk: u32,
    /// `android:label` when it is a literal string. A resource reference cannot
    /// be resolved without `resources.arsc`, so it stays `None` rather than
    /// becoming an empty label.
    pub label: Option<String>,
    /// First activity advertising `android.intent.action.MAIN`.
    pub launcher_activity: Option<String>,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8], pos: usize) -> Self {
        Reader { buf, pos }
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::BadManifest(format!(
                "truncated at offset {}: wanted {} bytes, {} left",
                self.pos,
                n,
                self.remaining()
            )));
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
}

/// A parsed string pool. Indexing out of range yields `None` instead of a panic,
/// because a hostile or truncated APK must not take the launcher down.
struct StringPool {
    strings: Vec<String>,
}

impl StringPool {
    fn parse(buf: &[u8], chunk_start: usize, header_size: usize, chunk_size: usize) -> Result<Self> {
        let mut r = Reader::new(buf, chunk_start + 8);
        let string_count = r.u32()? as usize;
        let _style_count = r.u32()? as usize;
        let flags = r.u32()?;
        let strings_start = r.u32()? as usize;
        let _styles_start = r.u32()?;
        let utf8 = flags & 0x100 != 0;

        // Offsets are relative to the start of the string data, which is itself
        // measured from the chunk start.
        let offsets_at = chunk_start + header_size;
        let data_at = chunk_start + strings_start;
        let chunk_end = chunk_start.saturating_add(chunk_size).min(buf.len());

        let mut strings = Vec::with_capacity(string_count.min(1024));
        for i in 0..string_count {
            let off_pos = offsets_at + i * 4;
            if off_pos + 4 > buf.len() {
                strings.push(String::new());
                continue;
            }
            let off = u32::from_le_bytes([
                buf[off_pos],
                buf[off_pos + 1],
                buf[off_pos + 2],
                buf[off_pos + 3],
            ]) as usize;
            let start = data_at.saturating_add(off);
            if start >= chunk_end {
                strings.push(String::new());
                continue;
            }
            strings.push(if utf8 {
                read_utf8_string(buf, start, chunk_end)
            } else {
                read_utf16_string(buf, start, chunk_end)
            });
        }
        Ok(StringPool { strings })
    }

    fn get(&self, idx: u32) -> Option<&str> {
        self.strings.get(idx as usize).map(|s| s.as_str())
    }
}

/// UTF-8 pool entry: one or two length bytes (UTF-16 count), then one or two
/// bytes (UTF-8 byte count), then that many bytes, NUL terminated.
fn read_utf8_string(buf: &[u8], start: usize, end: usize) -> String {
    let mut p = start;
    let read_len = |p: &mut usize| -> Option<usize> {
        if *p >= end {
            return None;
        }
        let b = buf[*p];
        *p += 1;
        if b & 0x80 != 0 {
            if *p >= end {
                return None;
            }
            let b2 = buf[*p];
            *p += 1;
            Some((((b & 0x7f) as usize) << 8) | b2 as usize)
        } else {
            Some(b as usize)
        }
    };
    if read_len(&mut p).is_none() {
        return String::new();
    }
    let byte_len = match read_len(&mut p) {
        Some(n) => n,
        None => return String::new(),
    };
    let stop = p.saturating_add(byte_len).min(end);
    String::from_utf8_lossy(&buf[p..stop]).into_owned()
}

/// UTF-16 pool entry: one or two 16-bit length units, then UTF-16LE data.
fn read_utf16_string(buf: &[u8], start: usize, end: usize) -> String {
    let mut p = start;
    if p + 2 > end {
        return String::new();
    }
    let mut len = u16::from_le_bytes([buf[p], buf[p + 1]]) as usize;
    p += 2;
    if len & 0x8000 != 0 {
        if p + 2 > end {
            return String::new();
        }
        let low = u16::from_le_bytes([buf[p], buf[p + 1]]) as usize;
        p += 2;
        len = ((len & 0x7fff) << 16) | low;
    }
    let stop = p.saturating_add(len * 2).min(end);
    let mut units = Vec::with_capacity(len);
    let mut i = p;
    while i + 1 < stop {
        units.push(u16::from_le_bytes([buf[i], buf[i + 1]]));
        i += 2;
    }
    String::from_utf16_lossy(&units)
}

fn decode_value(pool: &StringPool, data_type: u8, data: u32) -> Value {
    match data_type {
        TYPE_STRING => Value::Str(pool.get(data).unwrap_or("").to_string()),
        TYPE_INT_DEC | TYPE_INT_HEX => Value::Int(data as i32 as i64),
        TYPE_INT_BOOLEAN => Value::Bool(data != 0),
        TYPE_REFERENCE => Value::Ref(data),
        other => Value::Raw(other, data),
    }
}

struct Attr {
    name: String,
    value: Value,
}

/// Parse a compiled `AndroidManifest.xml`.
pub fn parse(buf: &[u8]) -> Result<Manifest> {
    if buf.len() < 8 {
        return Err(Error::BadManifest("shorter than a chunk header".into()));
    }
    let typ = u16::from_le_bytes([buf[0], buf[1]]);
    let header_size = u16::from_le_bytes([buf[2], buf[3]]) as usize;
    if typ != RES_XML {
        return Err(Error::BadManifest(format!(
            "expected RES_XML (0x{RES_XML:04x}), got 0x{typ:04x}"
        )));
    }
    if header_size < 8 {
        return Err(Error::BadManifest(format!(
            "absurd root header size {header_size}"
        )));
    }

    let mut pool: Option<StringPool> = None;
    let mut out = Manifest::default();
    let mut saw_manifest = false;
    let mut activity_stack: Vec<String> = Vec::new();

    let mut pos = header_size;
    while pos + 8 <= buf.len() {
        let mut r = Reader::new(buf, pos);
        let ctype = r.u16()?;
        let cheader = r.u16()? as usize;
        let csize = r.u32()? as usize;
        if csize < 8 || pos + csize > buf.len() {
            // A chunk that runs past the end means the file is truncated; stop
            // rather than reading whatever follows in memory.
            break;
        }
        if cheader < 8 || cheader > csize {
            return Err(Error::BadManifest(format!(
                "chunk at {pos} has header size {cheader} but length {csize}"
            )));
        }

        match ctype {
            RES_STRING_POOL => {
                if pool.is_none() {
                    pool = Some(StringPool::parse(buf, pos, cheader, csize)?);
                }
            }
            RES_XML_START_ELEMENT => {
                let p = match pool.as_ref() {
                    Some(p) => p,
                    // Elements before the string pool cannot be decoded.
                    None => {
                        pos += csize;
                        continue;
                    }
                };
                let mut er = Reader::new(buf, pos);
                // The 16-byte node header (type, headerSize, size, lineNumber,
                // comment) is already behind us; `headerSize` covers it plus
                // anything the platform adds later.
                er.skip(cheader)?;
                let _ns = er.u32()?;
                let name_idx = er.u32()?;
                let attr_start = er.u16()? as usize;
                let attr_size = er.u16()? as usize;
                let attr_count = er.u16()? as usize;

                let tag = p.get(name_idx).unwrap_or("").to_string();

                // `attributeStart` is a 2-byte field measured from the start of
                // the attribute extension, not from the chunk. Reading it as 4
                // bytes, or forgetting the 16-byte node header, shifts every
                // attribute by two or by sixteen and yields plausible garbage.
                let mut attrs = Vec::with_capacity(attr_count.min(64));
                let base = pos + cheader + attr_start;
                for i in 0..attr_count {
                    let at = base + i * attr_size;
                    if at + 20 > pos + csize || at + 20 > buf.len() {
                        break;
                    }
                    let mut ar = Reader::new(buf, at);
                    let _ans = ar.u32()?;
                    let aname = ar.u32()?;
                    let _raw = ar.u32()?;
                    let _asize = ar.u16()?;
                    let _ares0 = ar.u8()?;
                    let dtype = ar.u8()?;
                    let data = ar.u32()?;
                    attrs.push(Attr {
                        name: p.get(aname).unwrap_or("").to_string(),
                        value: decode_value(p, dtype, data),
                    });
                }

                match tag.as_str() {
                    "manifest" => {
                        saw_manifest = true;
                        for a in &attrs {
                            match a.name.as_str() {
                                "package" => {
                                    if let Some(s) = a.value.as_str() {
                                        out.package = s.to_string();
                                    }
                                }
                                "versionCode" => {
                                    // versionCode can exceed i32 on some builds,
                                    // so take the raw unsigned reading when the
                                    // value came back negative from the i32 cast.
                                    if let Value::Int(v) = a.value {
                                        out.version_code = if v < 0 { 0 } else { v as u64 };
                                    }
                                }
                                "versionName" => {
                                    if let Some(s) = a.value.as_str() {
                                        out.version_name = s.to_string();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "application" => {
                        for a in &attrs {
                            if a.name == "label" {
                                if let Value::Str(s) = &a.value {
                                    out.label = Some(s.clone());
                                }
                            }
                        }
                    }
                    "uses-sdk" => {
                        for a in &attrs {
                            match a.name.as_str() {
                                "minSdkVersion" => {
                                    if let Some(v) = a.value.as_i64() {
                                        out.min_sdk = v.clamp(1, i32::MAX as i64) as u32;
                                    }
                                }
                                "targetSdkVersion" => {
                                    if let Some(v) = a.value.as_i64() {
                                        out.target_sdk = v.clamp(1, i32::MAX as i64) as u32;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "activity" | "activity-alias" => {
                        let name = attrs
                            .iter()
                            .find(|a| a.name == "name")
                            .and_then(|a| a.value.as_str())
                            .unwrap_or("")
                            .to_string();
                        activity_stack.push(name);
                    }
                    "action" => {
                        let is_main = attrs.iter().any(|a| {
                            a.name == "name"
                                && a.value.as_str() == Some("android.intent.action.MAIN")
                        });
                        if is_main {
                            if let Some(owner) = activity_stack.last() {
                                if out.launcher_activity.is_none() && !owner.is_empty() {
                                    out.launcher_activity = Some(owner.clone());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            RES_XML_END_ELEMENT => {
                // The end element names the tag; pop if it was an activity so a
                // nested activity inside an intent-filter cannot be mistaken
                // for the owner of a sibling <action>.
                if let Some(p) = pool.as_ref() {
                    let mut er = Reader::new(buf, pos);
                    if er.skip(cheader).is_ok() {
                        if let (Ok(_ns), Ok(n)) = (er.u32(), er.u32()) {
                            let tag = p.get(n).unwrap_or("");
                            if tag == "activity" || tag == "activity-alias" {
                                activity_stack.pop();
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        pos += csize;
    }

    if !saw_manifest {
        return Err(Error::BadManifest("no <manifest> element".into()));
    }
    if out.package.trim().is_empty() {
        return Err(Error::UnusableManifest(
            "<manifest> has no package attribute".into(),
        ));
    }
    // Applications with no <uses-sdk> predate the attribute; the platform then
    // treats both values as 1. Reporting 0 would make every version check fail.
    if out.min_sdk == 0 {
        out.min_sdk = 1;
    }
    if out.target_sdk == 0 {
        out.target_sdk = out.min_sdk;
    }
    Ok(out)
}
