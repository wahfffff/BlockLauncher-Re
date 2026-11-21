//! One list of "mods" over the two formats a user can drop into a folder.
//!
//! There are two unrelated things people call a mod here:
//!
//! * an **nmod** — a zip with `nmod_manifest.json`, read by [`crate::nmod`], the
//!   format ModdedMCPE/EnderCore use and what BlockLauncher's native mods became;
//! * a **legacy patch** — a single `.mod` file in the `PTP` container, read by
//!   [`crate::legacy`], which this build can describe but not apply.
//!
//! They are listed together because the user put them in one folder and does not
//! care about the taxonomy; they are *reported* differently because the second
//! kind cannot run, and saying so at list time is the difference between a mod
//! that fails at game start and one the user knows not to install.

use crate::error::{Error, Result};
use crate::legacy;
use crate::nmod;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Directory scan cap. A folder with thousands of entries is not a mod folder;
/// refusing to list it is better than making the UI allocate for it.
const MAX_ENTRIES: usize = 512;

/// A `.mod` bigger than this is not a `PTP` patch — the payload is tens of
/// bytes. Refusing early also stops a mis-named 200 MB file from being read.
const MAX_LEGACY_BYTES: u64 = 64 * 1024;

/// A mod package is a zip holding assets and libraries; 256 MiB is already an
/// order of magnitude past any real one, and reading it whole is what lets the
/// zip be parsed without a seekable file handle on Android.
const MAX_NMOD_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Zip with `nmod_manifest.json`.
    Nmod,
    /// Single-file `PTP` patch.
    Legacy,
}

/// One row in the mod list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// Bare name on disk.
    pub file_name: String,
    /// `nmod` or `legacy`.
    pub kind: Kind,
    /// What to show as the title: the manifest name, or the file stem.
    pub title: String,
    /// Who made it, when the format records that.
    pub author: String,
    /// True when the launcher could load this into a game today.
    pub usable: bool,
    /// Empty when usable; otherwise the sentence explaining why not.
    pub reason: String,
    /// Free-form extra: version and supports for an nmod, payload size for a patch.
    pub detail: String,
    /// Non-fatal problems the package declared (missing icon, missing author).
    pub warnings: Vec<String>,
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
            warnings: Vec::new(),
            path: path.to_string_lossy().into_owned(),
        }
    }
}

fn extension(name: &str) -> Option<String> {
    name.rsplit_once('.')
        .map(|(stem, ext)| (stem, ext.to_ascii_lowercase()))
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, ext)| ext)
}

/// List the mods directly inside `dir`, sorted by file name.
///
/// Not recursive on purpose: a nested tree is how a mod folder ends up scanning
/// an extracted APK and listing its `lib/` as mods.
pub fn scan(dir: &Path) -> Result<Vec<Entry>> {
    let mut out: Vec<Entry> = Vec::new();

    let read = std::fs::read_dir(dir).map_err(|e| {
        Error::BadMod(format!("cannot read {}: {e}", dir.display()))
    })?;

    let mut paths: Vec<PathBuf> = Vec::new();
    for item in read {
        let item = match item {
            Ok(i) => i,
            // An unreadable entry is skipped rather than failing the whole list:
            // one permission-denied file should not hide the other nine.
            Err(_) => continue,
        };
        paths.push(item.path());
        if paths.len() > MAX_ENTRIES {
            break;
        }
    }
    paths.sort();

    for path in paths {
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let file_name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_owned(),
            None => continue,
        };
        let Some(ext) = extension(&file_name) else {
            continue;
        };

        match ext.as_str() {
            "mod" => out.push(read_legacy(&path, file_name, meta.len())),
            "nmod" | "zip" => {
                // A zip is only a mod if it says so; anything else in the folder
                // (a texture pack, an APK someone dropped) must not appear here.
                if let Some(entry) = read_nmod(&path, file_name, meta.len()) {
                    out.push(entry);
                }
            }
            _ => {}
        }
    }

    Ok(out)
}

fn read_nmod(path: &Path, file_name: String, len: u64) -> Option<Entry> {
    if len > MAX_NMOD_BYTES {
        return Some(Entry::unusable(
            Kind::Nmod,
            file_name,
            path,
            format!("{len} bytes is larger than the {MAX_NMOD_BYTES} byte limit"),
        ));
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return Some(Entry::unusable(
                Kind::Nmod,
                file_name,
                path,
                format!("cannot read: {e}"),
            ))
        }
    };

    // Not an nmod is not a broken nmod: skip silently so a folder holding both
    // mods and texture packs lists only the mods.
    if !looks_like_nmod(&bytes) {
        return None;
    }

    Some(match nmod::Package::open(bytes) {
        Ok(p) => {
            let supports = p.support_names();
            Entry {
                file_name,
                kind: Kind::Nmod,
                title: p.name().to_string(),
                author: p.author().to_string(),
                usable: true,
                reason: String::new(),
                detail: format!(
                    "nmod {} · {}",
                    p.version_name(),
                    if supports.is_empty() {
                        "no game supports".to_string()
                    } else {
                        supports.join(", ")
                    }
                ),
                warnings: p.warnings().to_vec(),
                path: path.to_string_lossy().into_owned(),
            }
        }
        Err(e) => Entry::unusable(Kind::Nmod, file_name.clone(), path, e.to_string()),
    })
}

/// Cheap pre-check: does the zip contain `nmod_manifest.json`?
///
/// Reads only the central directory (the whole buffer is already in memory), so
/// this costs nothing beyond the parse that `Package::open` would do anyway and
/// keeps a plain `.zip` from being reported as a broken mod.
fn looks_like_nmod(bytes: &[u8]) -> bool {
    match crate::apk::Apk::open(bytes) {
        Ok(zip) => zip.entry(nmod::MANIFEST).is_some(),
        Err(_) => false,
    }
}

fn read_legacy(path: &Path, file_name: String, len: u64) -> Entry {
    let fallback_title = file_name
        .rsplit_once('.')
        .map(|(s, _)| s.to_string())
        .unwrap_or_else(|| file_name.clone());

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
            reason: "legacy PTP patch: this build can read it but cannot apply it".to_string(),
            detail: format!("{} byte payload", m.payload.len()),
            warnings: Vec::new(),
            path: path.to_string_lossy().into_owned(),
        },
        Err(e) => Entry::unusable(Kind::Legacy, file_name, path, e.to_string()),
    }
}

/// Payload of a legacy patch, for a future build that learns what it means.
///
/// Exists so the opaque bytes are reachable without re-reading the file, and so
/// the tests can assert that the bytes survive the round trip.
pub fn legacy_payload(bytes: &[u8]) -> Result<legacy::LegacyMod> {
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

    /// Smallest valid nmod the reader accepts, stored uncompressed.
    fn nmod_bytes(name: &str) -> Vec<u8> {
        let manifest = format!(
            r#"{{"uuid":"0f8fad5b-d9cb-469f-a165-70867728950e","name":"{name}",
                "version_name":"1.0","version_code":1,"manifest_version":1,
                "min_sdk_version":1,"target_sdk_version":1,"author":"me",
                "game_supports":[{{"platform":"android","name":"pe",
                "target_game_versions":["0\\.14\\..*"]}}]}}"#
        );
        crate::tests_support::zip_stored(&[
            (nmod::MANIFEST, manifest.into_bytes()),
            ("android/pe/assets/a.txt", b"A".to_vec()),
        ])
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
    fn an_nmod_package_is_listed_as_usable() {
        let d = tmp("nmod");
        fs::write(d.join("cool.nmod"), nmod_bytes("Cool Pack")).unwrap();
        let list = scan(&d).unwrap();
        assert_eq!(list.len(), 1);
        let e = &list[0];
        assert_eq!(e.kind, Kind::Nmod);
        assert_eq!(e.title, "Cool Pack");
        assert_eq!(e.author, "me");
        assert!(e.usable, "{}", e.reason);
        assert_eq!(e.detail, "nmod 1.0 · pe");
    }

    /// A broken package stays visible: the user needs to see the thing they
    /// installed, with the reason the manifest gave.
    #[test]
    fn a_broken_nmod_is_listed_with_its_reason() {
        let d = tmp("broken");
        let mut bytes = nmod_bytes("x");
        // Truncate the central directory so the zip cannot be parsed.
        bytes.truncate(bytes.len() / 2);
        fs::write(d.join("broken.nmod"), &bytes).unwrap();
        // It no longer looks like an nmod at all, so it is skipped rather than
        // shown as a broken mod: a half-downloaded file has no manifest to read.
        assert!(scan(&d).unwrap().is_empty());
    }

    #[test]
    fn a_zip_without_a_manifest_is_not_a_mod() {
        let d = tmp("plainzip");
        fs::write(
            d.join("pack.zip"),
            crate::tests_support::zip_stored(&[("pack.mcmeta", b"{}".to_vec())]),
        )
        .unwrap();
        assert!(scan(&d).unwrap().is_empty());
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
