//! One list of "mods" over the two formats a user can drop into a folder.
//!
//! There are two unrelated things people call a mod here:
//!
//! * a **native mod** — a directory with `nmod.json` and a `.so` per ABI, read by
//!   [`crate::nmod`];
//! * a **legacy patch** — a single `.mod` file in the `PTP` container, read by
//!   [`crate::legacy`], which this build can describe but not apply.
//!
//! They are listed together because the user put them in one folder and does not
//! care about the taxonomy; they are *reported* differently because the second
//! kind cannot run, and saying so at list time is the difference between a mod
//! that fails at game start and one the user knows not to install.

use crate::error::Result;
use crate::legacy::{self, LegacyMod};
use crate::nmod::Nmod;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Directory scan cap. A folder with thousands of entries is not a mod folder;
/// refusing to list it is better than making the UI allocate for it.
const MAX_ENTRIES: usize = 512;

/// A `.mod` bigger than this is not a `PTP` patch — the payload is tens of
/// bytes. Refusing early also stops a mis-named 200 MB file from being read.
const MAX_LEGACY_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// `nmod.json` + `lib/<abi>/<entry>`.
    Native,
    /// Single-file `PTP` patch.
    Legacy,
}

/// One row in the mod list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// Bare name on disk: the directory name or the file name.
    pub file_name: String,
    /// `native` or `legacy`.
    pub kind: Kind,
    /// What to show as the title: the manifest name, the author, or the file stem.
    pub title: String,
    /// Who made it, when the format records that.
    pub author: String,
    /// True when the launcher could load this into a game today.
    pub usable: bool,
    /// Empty when usable; otherwise the sentence explaining why not.
    pub reason: String,
    /// Free-form extra: ABIs for a native mod, payload size for a patch.
    pub detail: String,
    pub path: String,
}

impl Entry {
    fn unusable(kind: Kind, file_name: String, path: &Path, reason: String) -> Entry {
        Entry {
            title: file_name.clone(),
            file_name,
            kind,
            author: String::new(),
            usable: false,
            reason,
            detail: String::new(),
            path: path.to_string_lossy().into_owned(),
        }
    }
}

/// List the mods directly inside `dir`, sorted by file name.
///
/// Not recursive on purpose: a nested tree is how a mod folder ends up scanning
/// an extracted APK and listing its `lib/` as mods.
pub fn scan(dir: &Path) -> Result<Vec<Entry>> {
    let mut out: Vec<Entry> = Vec::new();

    let read = std::fs::read_dir(dir).map_err(|e| {
        crate::error::Error::BadMod(format!("cannot read {}: {e}", dir.display()))
    })?;

    let mut names: Vec<PathBuf> = Vec::new();
    for item in read {
        let item = match item {
            Ok(i) => i,
            // An unreadable entry is skipped rather than failing the whole list:
            // one permission-denied file should not hide the other nine.
            Err(_) => continue,
        };
        names.push(item.path());
        if names.len() > MAX_ENTRIES {
            break;
        }
    }
    names.sort();

    for path in names {
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let file_name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_owned(),
            None => continue,
        };

        if meta.is_dir() {
            if !path.join(crate::nmod::MANIFEST_FILE).is_file() {
                continue;
            }
            out.push(read_native(&path, file_name));
            continue;
        }

        if !is_legacy_name(&file_name) {
            continue;
        }
        out.push(read_legacy(&path, file_name, meta.len()));
    }

    Ok(out)
}

fn is_legacy_name(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(stem, ext)| !stem.is_empty() && ext.eq_ignore_ascii_case("mod"))
        .unwrap_or(false)
}

fn stem(name: &str) -> String {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name).to_owned()
}

fn read_native(root: &Path, file_name: String) -> Entry {
    match Nmod::load(root) {
        Ok(n) => Entry {
            file_name,
            kind: Kind::Native,
            title: n.manifest.name.clone(),
            author: String::new(),
            usable: true,
            reason: String::new(),
            detail: format!("native for {}", n.manifest.abis.join(", ")),
            path: root.to_string_lossy().into_owned(),
        },
        Err(e) => Entry::unusable(
            Kind::Native,
            file_name.clone(),
            root,
            e.to_string(),
        ),
    }
}

fn read_legacy(path: &Path, file_name: String, len: u64) -> Entry {
    let fallback_title = stem(&file_name);

    if len > MAX_LEGACY_BYTES {
        return Entry::unusable(
            Kind::Legacy,
            file_name,
            path,
            format!("{len} bytes is too large to be a PTP patch"),
        );
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return Entry::unusable(Kind::Legacy, file_name, path, format!("cannot read: {e}"))
        }
    };

    match legacy::parse(&bytes) {
        Ok(m) => Entry {
            file_name,
            kind: Kind::Legacy,
            title: fallback_title,
            author: m.author().unwrap_or("").to_owned(),
            // Describable, not runnable. This is the one place the two are
            // separated, and the reason is repeated on the row rather than left
            // to the documentation.
            usable: false,
            reason: "legacy PTP patch: this build can read it but cannot apply it"
                .to_string(),
            detail: format!("{} byte payload", m.payload.len()),
            path: path.to_string_lossy().into_owned(),
        },
        Err(e) => Entry::unusable(Kind::Legacy, file_name, path, e.to_string()),
    }
}

/// Payload of a legacy patch, for a future build that learns what it means.
///
/// Exists so the opaque bytes are reachable without re-reading the file, and so
/// the tests can assert that the bytes survive the round trip.
pub fn legacy_payload(bytes: &[u8]) -> Result<LegacyMod> {
    legacy::parse(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("blre-mods-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_mod(dir: &Path, name: &str) -> PathBuf {
        let bytes = [
            0xffu8, 0x50, 0x54, 0x50, 0x00, 0x01, 0x00, 0x00, 0x00, 0x17, 0x4d, 0x61, 0x64, 0x65,
            0x20, 0x62, 0x79, 0x20, 0x4b, 0x53, 0x59, 0x4d, 0x43, 0x00, 0x10, 0x51, 0x32, 0xff,
        ];
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    /// The shape a native mod directory has to have for `Nmod::load` to accept it.
    fn write_native(dir: &Path, name: &str, id: &str) -> PathBuf {
        let root = dir.join(name);
        fs::create_dir_all(root.join("lib/arm64-v8a")).unwrap();
        fs::write(
            root.join("nmod.json"),
            format!(
                r#"{{"schema":1,"id":"{id}","name":"Test mod","version":"1.0","entry":"libtest.so","abis":["arm64-v8a"]}}"#
            ),
        )
        .unwrap();
        fs::write(root.join("lib/arm64-v8a/libtest.so"), b"\x7fELF").unwrap();
        root
    }

    #[test]
    fn an_empty_directory_lists_nothing() {
        let d = tmp("empty");
        assert!(scan(&d).unwrap().is_empty());
    }

    #[test]
    fn a_legacy_patch_is_listed_as_usable_false() {
        let d = tmp("legacy");
        write_mod(&d, "夜视.mod");
        let list = scan(&d).unwrap();
        assert_eq!(list.len(), 1);
        let e = &list[0];
        assert_eq!(e.kind, Kind::Legacy);
        assert_eq!(e.title, "夜视");
        assert_eq!(e.author, "KSYMC");
        assert!(!e.usable);
        assert!(e.reason.contains("cannot apply"), "{}", e.reason);
        assert_eq!(e.detail, "4 byte payload");
    }

    #[test]
    fn a_native_mod_is_listed_as_usable() {
        let d = tmp("native");
        write_native(&d, "coolmod", "coolmod");
        let list = scan(&d).unwrap();
        assert_eq!(list.len(), 1);
        let e = &list[0];
        assert_eq!(e.kind, Kind::Native);
        assert_eq!(e.title, "Test mod");
        assert!(e.usable);
        assert!(e.reason.is_empty());
        assert_eq!(e.detail, "native for arm64-v8a");
    }

    /// A native mod that promises an ABI it does not ship must appear in the
    /// list with the reason, not vanish — the user needs to see the broken thing
    /// they installed.
    #[test]
    fn a_broken_native_mod_is_listed_with_its_reason() {
        let d = tmp("brokennative");
        let root = write_native(&d, "half", "half");
        fs::remove_file(root.join("lib/arm64-v8a/libtest.so")).unwrap();
        let list = scan(&d).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, Kind::Native);
        assert!(!list[0].usable);
        assert!(list[0].reason.contains("missing"), "{}", list[0].reason);
    }

    #[test]
    fn a_corrupt_legacy_patch_is_listed_with_its_reason() {
        let d = tmp("corrupt");
        fs::write(d.join("broken.mod"), b"not a patch").unwrap();
        let list = scan(&d).unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].usable);
        assert!(list[0].reason.contains("expected ff 50 54 50"), "{}", list[0].reason);
    }

    #[test]
    fn unrelated_files_are_ignored() {
        let d = tmp("ignore");
        fs::write(d.join("readme.txt"), b"hello").unwrap();
        fs::write(d.join("pack.zip"), b"PK\x03\x04").unwrap();
        fs::create_dir_all(d.join("plain-dir")).unwrap();
        assert!(scan(&d).unwrap().is_empty());
    }

    /// `.MOD` and `.Mod` are the same extension; a case-sensitive check would
    /// silently drop a file the user can see on disk.
    #[test]
    fn the_extension_is_matched_case_insensitively() {
        let d = tmp("case");
        write_mod(&d, "thing.MOD");
        let list = scan(&d).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "thing");
    }

    #[test]
    fn entries_are_sorted_by_file_name() {
        let d = tmp("sorted");
        write_mod(&d, "b.mod");
        write_mod(&d, "a.mod");
        write_mod(&d, "c.mod");
        let names: Vec<_> = scan(&d).unwrap().into_iter().map(|e| e.file_name).collect();
        assert_eq!(names, vec!["a.mod", "b.mod", "c.mod"]);
    }

    #[test]
    fn a_missing_directory_is_an_error_not_an_empty_list() {
        let d = tmp("missing");
        std::fs::remove_dir_all(&d).unwrap();
        assert!(scan(&d).is_err());
    }

    #[test]
    fn an_oversized_mod_file_is_refused_without_reading_it() {
        let d = tmp("huge");
        let p = d.join("huge.mod");
        fs::write(&p, vec![0u8; (MAX_LEGACY_BYTES + 1) as usize]).unwrap();
        let list = scan(&d).unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].usable);
        assert!(list[0].reason.contains("too large"), "{}", list[0].reason);
    }
}
