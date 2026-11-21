//! Just enough of the ZIP format to read an APK's manifest and list its ABIs.
//!
//! A launcher opens APKs that were produced by a dozen different toolchains and
//! patched by a dozen different people, so this reader refuses to trust any
//! length it finds: every offset is checked against the buffer before use, and
//! an archive that contradicts itself is reported instead of sliced.

use crate::axml::{self, Manifest};
use crate::error::{Error, Result};

const SIG_EOCD: u32 = 0x0605_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_LOCAL: u32 = 0x0403_4b50;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub local_offset: u32,
}

#[derive(Debug, Clone)]
pub struct Apk<'a> {
    buf: &'a [u8],
    entries: Vec<Entry>,
}

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32le(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

impl<'a> Apk<'a> {
    pub fn open(buf: &'a [u8]) -> Result<Self> {
        let entries = Self::read_central_directory(buf)?;
        Ok(Apk { buf, entries })
    }

    fn read_central_directory(buf: &[u8]) -> Result<Vec<Entry>> {
        if buf.len() < 22 {
            return Err(Error::NotZip(format!("{} bytes is too short", buf.len())));
        }

        // The end-of-central-directory record sits at the end, after an optional
        // comment of up to 64 KiB. Scan backwards for its signature.
        let scan_from = buf.len().saturating_sub(22 + 0xffff);
        let mut eocd = None;
        let mut i = buf.len() - 22;
        loop {
            if u32le(buf, i) == Some(SIG_EOCD) {
                eocd = Some(i);
                break;
            }
            if i == scan_from {
                break;
            }
            i -= 1;
        }
        let eocd = eocd.ok_or_else(|| Error::NotZip("no end-of-central-directory record".into()))?;

        let total = u16le(buf, eocd + 10).ok_or_else(|| Error::NotZip("truncated EOCD".into()))? as u32;
        let cd_size = u32le(buf, eocd + 12).ok_or_else(|| Error::NotZip("truncated EOCD".into()))?;
        let cd_offset = u32le(buf, eocd + 16).ok_or_else(|| Error::NotZip("truncated EOCD".into()))?;

        if total == 0xffff || cd_size == 0xffff_ffff || cd_offset == 0xffff_ffff {
            // ZIP64 exists for archives over 4 GiB. No APK is, and guessing at
            // the ZIP64 locator would turn a clear refusal into a wrong answer.
            return Err(Error::NotZip(
                "ZIP64 archives are not supported (an APK should never need one)".into(),
            ));
        }

        let cd_end = (cd_offset as usize)
            .checked_add(cd_size as usize)
            .ok_or_else(|| Error::NotZip("central directory runs past the file".into()))?;
        if cd_end > buf.len() {
            return Err(Error::NotZip(format!(
                "central directory ends at {cd_end}, file is {} bytes",
                buf.len()
            )));
        }

        let mut entries = Vec::with_capacity((total as usize).min(4096));
        let mut p = cd_offset as usize;
        for _ in 0..total {
            if u32le(buf, p) != Some(SIG_CENTRAL) {
                return Err(Error::NotZip(format!(
                    "central directory entry at {p} has no signature"
                )));
            }
            let method = u16le(buf, p + 10).unwrap_or(0);
            let csize = u32le(buf, p + 20).unwrap_or(0);
            let usize_ = u32le(buf, p + 24).unwrap_or(0);
            let name_len = u16le(buf, p + 28).unwrap_or(0) as usize;
            let extra_len = u16le(buf, p + 30).unwrap_or(0) as usize;
            let comment_len = u16le(buf, p + 32).unwrap_or(0) as usize;
            let local_offset = u32le(buf, p + 42).unwrap_or(0);

            let name_bytes = buf
                .get(p + 46..p + 46 + name_len)
                .ok_or_else(|| Error::NotZip("entry name runs past the file".into()))?;
            let name = String::from_utf8_lossy(name_bytes).into_owned();

            entries.push(Entry {
                name,
                method,
                compressed_size: csize,
                uncompressed_size: usize_,
                local_offset,
            });

            p += 46 + name_len + extra_len + comment_len;
            if p > cd_end {
                break;
            }
        }
        Ok(entries)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Decompress one entry, borrowing the archive buffer.
    pub fn read(&self, e: &Entry) -> Result<Vec<u8>> {
        let at = e.local_offset as usize;
        if u32le(self.buf, at) != Some(SIG_LOCAL) {
            return Err(Error::NotZip(format!(
                "entry '{}' does not point at a local header",
                e.name
            )));
        }
        let name_len = u16le(self.buf, at + 26).unwrap_or(0) as usize;
        let extra_len = u16le(self.buf, at + 28).unwrap_or(0) as usize;
        let data_at = at
            .checked_add(30)
            .and_then(|v| v.checked_add(name_len))
            .and_then(|v| v.checked_add(extra_len))
            .ok_or_else(|| Error::NotZip("entry offsets overflow".into()))?;

        let csize = e.compressed_size as usize;
        let data = self.buf.get(data_at..data_at + csize).ok_or_else(|| {
            Error::NotZip(format!("entry '{}' data runs past the file", e.name))
        })?;

        match e.method {
            0 => Ok(data.to_vec()),
            8 => miniz_oxide::inflate::decompress_to_vec(data).map_err(|err| {
                Error::NotZip(format!("entry '{}' failed to inflate: {err:?}", e.name))
            }),
            other => Err(Error::NotZip(format!(
                "entry '{}' uses compression method {other}, which this reader does not implement",
                e.name
            ))),
        }
    }
}

/// ABIs an APK ships native code for, ordered by what Android prefers on a
/// 64-bit device. An APK with no `lib/` at all is a pure-Java build and gets an
/// empty list rather than a fabricated "universal" entry.
pub fn abis(apk: &Apk<'_>) -> Vec<String> {
    const ORDER: [&str; 4] = ["arm64-v8a", "armeabi-v7a", "x86_64", "x86"];
    let mut found: Vec<String> = Vec::new();
    for name in apk.names() {
        let rest = match name.strip_prefix("lib/") {
            Some(r) => r,
            None => continue,
        };
        let mut parts = rest.split('/');
        let abi = match parts.next() {
            Some(a) => a,
            None => continue,
        };
        let file = match parts.next() {
            Some(f) => f,
            None => continue,
        };
        if !file.ends_with(".so") || parts.next().is_some() {
            continue;
        }
        if !found.iter().any(|f| f == abi) {
            found.push(abi.to_string());
        }
    }
    found.sort_by_key(|a| {
        ORDER
            .iter()
            .position(|o| o == a)
            .unwrap_or(ORDER.len())
    });
    found
}

/// Everything the launcher records about one game build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApkInfo {
    pub manifest: Manifest,
    pub abis: Vec<String>,
    pub has_manifest: bool,
}

/// Inspect an APK held in memory.
///
/// The manifest is mandatory — an archive without one is not an APK. ABIs are
/// optional, because a pure-Java build genuinely has none.
pub fn inspect(buf: &[u8]) -> Result<ApkInfo> {
    let apk = Apk::open(buf)?;
    let manifest_entry = apk
        .entry("AndroidManifest.xml")
        .ok_or_else(|| Error::MissingEntry("AndroidManifest.xml".into()))?;
    let raw = apk.read(manifest_entry)?;
    let manifest = axml::parse(&raw)?;
    let abis = abis(&apk);
    Ok(ApkInfo {
        manifest,
        abis,
        has_manifest: true,
    })
}
