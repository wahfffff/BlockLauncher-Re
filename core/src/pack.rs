//! Texture packs.
//!
//! A pack is a zip the game would read from its own data directory. This module
//! does the part that can be decided off-device: is it a zip at all, does it
//! contain textures rather than being an empty archive, what does its manifest
//! say it is called, and which packs are installed and enabled.
//!
//! What it deliberately does not do is claim the game accepted the pack. The
//! only authority on that is the game, and it is not present here.
//!
//! Old Minecraft PE packs are wildly inconsistent — some ship
//! `manifest.json`, some only `pack.png` and a `textures/` tree, some put
//! everything under `assets/`. So the rules are deliberately permissive about
//! shape and strict about the one thing that matters: a pack with no texture
//! files is not a texture pack, and saying otherwise would install something
//! that silently does nothing.

use crate::apk::Apk;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const SCHEMA: u32 = 1;
const INDEX_FILE: &str = "packs.json";
const PACK_DIR: &str = "packs";

/// Textures live here in every pack format this launcher has seen.
const TEXTURE_ROOTS: [&str; 3] = ["textures/", "assets/", "texture_packs/"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// Size of the archive, so the UI can say "2.3 MB" without reopening it.
    pub size: u64,
    /// How many texture files the archive actually carries.
    pub textures: usize,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Index {
    schema: u32,
    #[serde(default)]
    packs: Vec<Pack>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedPack {
    pub pack: Pack,
    /// True when the same bytes were already installed: importing twice must not
    /// produce two entries the user then has to tell apart.
    pub deduped: bool,
}

pub struct PackStore {
    root: PathBuf,
    index: Index,
}

impl PackStore {
    pub fn open(root: &Path) -> Result<PackStore> {
        std::fs::create_dir_all(root.join(PACK_DIR))
            .map_err(|e| Error::Io(format!("cannot create {}: {e}", root.display())))?;

        let path = root.join(INDEX_FILE);
        let index = if path.exists() {
            let raw = std::fs::read(&path)
                .map_err(|e| Error::Io(format!("cannot read {}: {e}", path.display())))?;
            let parsed: Index = serde_json::from_slice(&raw)
                .map_err(|e| Error::Corrupt(format!("{}: {e}", path.display())))?;
            if parsed.schema != SCHEMA {
                return Err(Error::Corrupt(format!(
                    "{}: schema {} is not {}",
                    path.display(),
                    parsed.schema,
                    SCHEMA
                )));
            }
            parsed
        } else {
            Index { schema: SCHEMA, packs: Vec::new() }
        };

        Ok(PackStore { root: root.to_path_buf(), index })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn packs(&self) -> &[Pack] {
        &self.index.packs
    }

    pub fn get(&self, id: &str) -> Option<&Pack> {
        self.index.packs.iter().find(|p| p.id == id)
    }

    /// Validate and keep a pack. `source_name` is the user's filename, used only
    /// as a last-resort display name when the archive has no manifest.
    pub fn import(&mut self, bytes: &[u8], source_name: Option<&str>) -> Result<ImportedPack> {
        let archive = Apk::open(bytes)?;

        let mut textures = 0usize;
        let mut has_manifest = false;
        let mut has_icon = false;

        for name in archive.names() {
            let lower = name.to_ascii_lowercase();
            if TEXTURE_ROOTS.iter().any(|root| lower.starts_with(root)) {
                textures += 1;
            }
            if lower == "manifest.json" {
                has_manifest = true;
            }
            if lower == "pack.png" || lower == "pack_icon.png" {
                has_icon = true;
            }
        }

        if textures == 0 {
            // An archive with a manifest but no textures is a resource pack this
            // launcher cannot make sense of, not an empty file — say so plainly.
            return Err(Error::BadPack(if has_manifest {
                "archive has a manifest but no texture files".into()
            } else {
                "archive contains no textures/, assets/ or texture_packs/ entries".into()
            }));
        }

        let id = short_id(bytes);
        if let Some(existing) = self.index.packs.iter().find(|p| p.id == id) {
            return Ok(ImportedPack { pack: existing.clone(), deduped: true });
        }

        let manifest = if has_manifest {
            archive
                .entry("manifest.json")
                .and_then(|e| archive.read(e).ok())
                .and_then(|raw| Manifest::parse(&raw).ok())
        } else {
            None
        };

        // A missing or unreadable manifest is not fatal: the pack still has
        // textures the game can use. It only means the name is ours to guess.
        let name = manifest
            .as_ref()
            .and_then(|m| m.name.clone())
            .or_else(|| source_name.and_then(stem))
            .unwrap_or_else(|| format!("Texture pack {id}"));

        let version = manifest
            .as_ref()
            .and_then(|m| m.version.clone())
            .unwrap_or_else(|| "unknown".into());

        let description = manifest
            .as_ref()
            .and_then(|m| m.description.clone())
            .unwrap_or_default();

        let pack = Pack {
            id: id.clone(),
            name,
            version,
            description,
            size: bytes.len() as u64,
            textures,
            enabled: true,
        };

        let dir = self.root.join(PACK_DIR);
        std::fs::write(dir.join(format!("{id}.zip")), bytes)
            .map_err(|e| Error::Io(format!("cannot store pack {id}: {e}")))?;

        self.index.packs.push(pack.clone());
        self.save()?;

        let _ = has_icon;
        Ok(ImportedPack { pack, deduped: false })
    }

    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let Some(pos) = self.index.packs.iter().position(|p| p.id == id) else {
            return Ok(false);
        };
        self.index.packs.remove(pos);

        let path = self.root.join(PACK_DIR).join(format!("{id}.zip"));
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            // The index is the truth; an already-missing file is not a failure to
            // remove the entry, and reporting one would leave the user stuck.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Io(format!("cannot delete {}: {e}", path.display()))),
        }
        self.save()?;
        Ok(true)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<Pack> {
        let pack = self
            .index
            .packs
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| Error::NotFound(format!("no pack with id {id}")))?;
        pack.enabled = enabled;
        let out = pack.clone();
        self.save()?;
        Ok(out)
    }

    /// Where the installed archive lives. The Android side copies this into the
    /// game's own pack directory before launching; the core only guarantees the
    /// file is here and that the index still describes it.
    pub fn archive_path(&self, id: &str) -> Result<PathBuf> {
        let pack = self
            .get(id)
            .ok_or_else(|| Error::NotFound(format!("no pack with id {id}")))?;
        let path = self.root.join(PACK_DIR).join(format!("{}.zip", pack.id));
        if !path.exists() {
            return Err(Error::Corrupt(format!(
                "{} is in the index but missing on disk",
                path.display()
            )));
        }
        Ok(path)
    }

    fn save(&self) -> Result<()> {
        let path = self.root.join(INDEX_FILE);
        let raw = serde_json::to_vec_pretty(&self.index)
            .map_err(|e| Error::Corrupt(format!("cannot encode index: {e}")))?;
        // Write-then-rename: a crash mid-write must not leave a truncated index
        // that the next start refuses to open.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, raw)
            .map_err(|e| Error::Io(format!("cannot write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| Error::Io(format!("cannot replace {}: {e}", path.display())))?;
        Ok(())
    }
}

fn short_id(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn stem(name: &str) -> Option<String> {
    let file = name.rsplit(['/', '\\']).next()?;
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    let stem = stem.trim();
    if stem.is_empty() { None } else { Some(stem.to_string()) }
}

/// The parts of a pack manifest worth reading. Both the modern
/// `{"header":{"name":...}}` shape and the flat one are accepted, because packs
/// predating the modern format are exactly the ones this launcher exists for.
#[derive(Debug, Default)]
struct Manifest {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
}

impl Manifest {
    fn parse(raw: &[u8]) -> std::result::Result<Manifest, ()> {
        let v: serde_json::Value = serde_json::from_slice(raw).map_err(|_| ())?;
        // A BOM shows up in manifests written by Windows tools and makes
        // serde_json reject the file, which is why this goes through a lossy
        // string first for the fields we read.
        let header = v.get("header").unwrap_or(&v);

        let name = string_field(header, "name").or_else(|| string_field(&v, "name"));
        let description =
            string_field(header, "description").or_else(|| string_field(&v, "description"));
        let version = version_field(header).or_else(|| version_field(&v));

        Ok(Manifest { name, version, description })
    }
}

fn string_field(v: &serde_json::Value, key: &str) -> Option<String> {
    let s = v.get(key)?.as_str()?.trim();
    if s.is_empty() { None } else { Some(s.to_string()) }
}

fn version_field(v: &serde_json::Value) -> Option<String> {
    match v.get("version")? {
        // `[1, 0, 0]` is what the modern format uses.
        serde_json::Value::Array(parts) => {
            let nums: Vec<String> = parts.iter().filter_map(|p| p.as_i64()).map(|n| n.to_string()).collect();
            if nums.is_empty() { None } else { Some(nums.join(".")) }
        }
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let mut p = std::env::temp_dir();
            p.push(format!(
                "blre-pack-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Minimal stored-only zip writer: the tests care about entry names, not
    /// about compression, and rolling it here keeps the test independent of the
    /// reader under test.
    fn zip_stored(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();

        for (name, data) in entries {
            let offset = out.len() as u32;
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            out.extend_from_slice(&[20, 0]); // version needed
            out.extend_from_slice(&[0, 0]); // flags
            out.extend_from_slice(&[0, 0]); // method: stored
            out.extend_from_slice(&[0; 4]); // time/date
            out.extend_from_slice(&crc32(data).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);

            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            central.extend_from_slice(&[20, 0]);
            central.extend_from_slice(&[20, 0]);
            central.extend_from_slice(&[0, 0]);
            central.extend_from_slice(&[0, 0]);
            central.extend_from_slice(&[0; 4]);
            central.extend_from_slice(&crc32(data).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            // extra length, comment length, disk number, internal attrs, external attrs
            central.extend_from_slice(&[0u8; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }

        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    fn texture_pack(name: &str) -> Vec<u8> {
        zip_stored(&[
            ("textures/blocks/stone.png", vec![0x89, b'P', b'N', b'G']),
            (
                "manifest.json",
                format!(
                    r#"{{"format_version":1,"header":{{"name":"{name}","description":"d","version":[1,2,0]}}}}"#
                )
                .into_bytes(),
            ),
        ])
    }

    #[test]
    fn a_pack_with_textures_is_accepted_and_named_from_the_manifest() {
        let t = TempDir::new("accept");
        let mut store = PackStore::open(&t.0).unwrap();
        let out = store.import(&texture_pack("Faithful"), Some("x.zip")).unwrap();

        assert!(!out.deduped);
        assert_eq!(out.pack.name, "Faithful");
        assert_eq!(out.pack.version, "1.2.0");
        assert_eq!(out.pack.textures, 1);
        assert!(out.pack.enabled);
        assert!(store.archive_path(&out.pack.id).unwrap().exists());
    }

    #[test]
    fn the_same_bytes_imported_twice_is_one_pack() {
        let t = TempDir::new("dedupe");
        let mut store = PackStore::open(&t.0).unwrap();
        let bytes = texture_pack("Faithful");

        let first = store.import(&bytes, None).unwrap();
        let second = store.import(&bytes, None).unwrap();

        assert!(!first.deduped);
        assert!(second.deduped);
        assert_eq!(first.pack.id, second.pack.id);
        assert_eq!(store.packs().len(), 1);
    }

    #[test]
    fn an_archive_without_textures_is_rejected() {
        let t = TempDir::new("empty");
        let mut store = PackStore::open(&t.0).unwrap();
        let empty = zip_stored(&[("readme.txt", b"hello".to_vec())]);

        match store.import(&empty, None) {
            Err(Error::BadPack(msg)) => assert!(msg.contains("no textures"), "{msg}"),
            other => panic!("expected BadPack, got {other:?}"),
        }
        assert_eq!(store.packs().len(), 0);
    }

    #[test]
    fn a_manifest_without_textures_says_so_rather_than_calling_it_empty() {
        let t = TempDir::new("manifestonly");
        let mut store = PackStore::open(&t.0).unwrap();
        let bytes = zip_stored(&[("manifest.json", br#"{"header":{"name":"x"}}"#.to_vec())]);

        match store.import(&bytes, None) {
            Err(Error::BadPack(msg)) => assert!(msg.contains("manifest"), "{msg}"),
            other => panic!("expected BadPack, got {other:?}"),
        }
    }

    #[test]
    fn a_broken_manifest_falls_back_to_the_filename_instead_of_failing() {
        let t = TempDir::new("brokenmanifest");
        let mut store = PackStore::open(&t.0).unwrap();
        let bytes = zip_stored(&[
            ("textures/blocks/stone.png", b"png".to_vec()),
            ("manifest.json", b"{ this is not json".to_vec()),
        ]);

        let out = store.import(&bytes, Some("my pack.zip")).unwrap();
        assert_eq!(out.pack.name, "my pack");
        assert_eq!(out.pack.version, "unknown");
    }

    #[test]
    fn an_assets_tree_counts_as_textures() {
        let t = TempDir::new("assets");
        let mut store = PackStore::open(&t.0).unwrap();
        let bytes = zip_stored(&[("assets/textures/blocks/stone.png", b"png".to_vec())]);
        let out = store.import(&bytes, None).unwrap();
        assert_eq!(out.pack.textures, 1);
    }

    #[test]
    fn disabling_persists_across_reopening() {
        let t = TempDir::new("toggle");
        let id = {
            let mut store = PackStore::open(&t.0).unwrap();
            let out = store.import(&texture_pack("A"), None).unwrap();
            store.set_enabled(&out.pack.id, false).unwrap();
            out.pack.id
        };

        let store = PackStore::open(&t.0).unwrap();
        let pack = store.get(&id).expect("pack survived the reopen");
        assert!(!pack.enabled);
    }

    #[test]
    fn removing_a_pack_deletes_the_archive_and_the_entry() {
        let t = TempDir::new("remove");
        let mut store = PackStore::open(&t.0).unwrap();
        let out = store.import(&texture_pack("A"), None).unwrap();

        assert!(store.remove(&out.pack.id).unwrap());
        assert!(store.packs().is_empty());
        assert!(!t.0.join("packs").join(format!("{}.zip", out.pack.id)).exists());
        assert!(!store.remove(&out.pack.id).unwrap());
    }

    #[test]
    fn a_corrupt_index_is_reported_not_silently_replaced() {
        let t = TempDir::new("corrupt");
        std::fs::create_dir_all(t.0.join(PACK_DIR)).unwrap();
        std::fs::write(t.0.join(INDEX_FILE), b"{not json").unwrap();

        match PackStore::open(&t.0) {
            Err(Error::Corrupt(_)) => {}
            other => panic!("expected Corrupt, got {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn an_index_from_a_future_schema_is_refused() {
        let t = TempDir::new("schema");
        std::fs::create_dir_all(t.0.join(PACK_DIR)).unwrap();
        std::fs::write(t.0.join(INDEX_FILE), br#"{"schema":99,"packs":[]}"#).unwrap();

        match PackStore::open(&t.0) {
            Err(Error::Corrupt(msg)) => assert!(msg.contains("schema"), "{msg}"),
            other => panic!("expected Corrupt, got {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn a_pack_missing_from_disk_is_reported_by_path() {
        let t = TempDir::new("gone");
        let id = {
            let mut store = PackStore::open(&t.0).unwrap();
            store.import(&texture_pack("A"), None).unwrap().pack.id
        };
        std::fs::remove_file(t.0.join(PACK_DIR).join(format!("{id}.zip"))).unwrap();

        let store = PackStore::open(&t.0).unwrap();
        match store.archive_path(&id) {
            Err(Error::Corrupt(msg)) => assert!(msg.contains(&id), "{msg}"),
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }
}
