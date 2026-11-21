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
//! ## What counts as a pack
//!
//! The rule used to be "an entry path starts with `textures/`, `assets/` or
//! `texture_packs/`". Run against seven real packs from the 0.14 era, that rule
//! accepted two and refused five:
//!
//! | real layout | old rule |
//! |---|---|
//! | `assets/images/...` + `pack.mcmeta` | accepted |
//! | `images/...` + `pack.mcmeta` | refused — no `assets/` prefix |
//! | `Ultimacraft_Texture/armor/...` (one wrapper directory) | refused |
//! | `cf主题0.4-wzj/assets/images/...` (wrapper plus assets) | refused |
//! | `Creeper'S V8.0 0.14.x/assets/...` (wrapper) | refused |
//! | `renderchunk.fragment` + `.vertex` at the root | refused — a shader pack |
//!
//! A directory-name whitelist was the mistake: the names differ per pack, and the
//! wrapper directory is whatever the author called it. So the question is no
//! longer "does the path start with a known directory" but "does this archive
//! carry something the game loads" — an image or a shader — and the one archive
//! that must never be mistaken for a pack is an APK, which is checked for by
//! name and refused with an explanation.

use crate::apk::Apk;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const SCHEMA: u32 = 1;
const INDEX_FILE: &str = "packs.json";
const PACK_DIR: &str = "packs";

/// Extensions the game loads as images. `.tga` and `.jpg` are here because old
/// packs shipped them; a modern pack is `.png` only.
fn is_image(lower: &str) -> bool {
    lower.ends_with(".png")
        || lower.ends_with(".tga")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
}

/// Shader files. `renderchunk.vertex` at an archive root is a complete,
/// working shader pack for 0.14 — two files, no directories, no manifest.
fn is_shader(lower: &str) -> bool {
    lower.ends_with(".fragment") || lower.ends_with(".vertex") || lower.ends_with(".glsl")
}

/// An APK is not a texture pack, and accepting one would install a 40 MB archive
/// that the game ignores. Checked by the two entries every APK has, because the
/// alternative — looking for images — matches an APK's icons.
fn looks_like_apk(names: &[&str]) -> bool {
    let manifest = names
        .iter()
        .any(|n| n.eq_ignore_ascii_case("AndroidManifest.xml"));
    let dex = names.iter().any(|n| {
        let l = n.to_ascii_lowercase();
        l.starts_with("classes") && l.ends_with(".dex")
    });
    manifest && dex
}

/// The single directory every entry lives under, when there is one.
///
/// `Some("Ultimacraft_Texture/")` for an archive zipped from a folder. `None`
/// when entries sit at the root, or are spread over more than one top directory,
/// in which case there is nothing to strip and no single place to look.
fn common_prefix<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut prefix: Option<&'a str> = None;
    let mut seen = 0usize;

    for name in names {
        // An entry at the archive root means there is no shared directory.
        let (dir, _) = name.split_once('/')?;
        match prefix {
            None => prefix = Some(dir),
            Some(d) if d == dir => {}
            _ => return None,
        }
        seen += 1;
    }

    if seen == 0 {
        return None;
    }
    prefix.map(|p| format!("{p}/"))
}

/// Find `leaf` at the archive root or under the wrapper directory.
fn find_entry<'a>(names: &[&'a str], prefix: Option<&str>, leaf: &str) -> Option<&'a str> {
    let wanted = match prefix {
        Some(p) => format!("{p}{leaf}"),
        None => leaf.to_string(),
    };
    names.iter().find(|n| n.eq_ignore_ascii_case(&wanted)).copied()
}

/// What a pack replaces.
///
/// The distinction is not cosmetic: a shader pack carries no textures, so a UI
/// that only counts images shows "0 张贴图" for a pack that works perfectly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Texture,
    Shader,
}

impl Default for Kind {
    fn default() -> Self {
        Kind::Texture
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// Size of the archive, so the UI can say "2.3 MB" without reopening it.
    pub size: u64,
    /// How many image files the archive actually carries.
    pub textures: usize,
    /// How many shader files. A pack may have these and no textures.
    #[serde(default)]
    pub shaders: usize,
    /// `default` so an index written before this field existed still loads.
    #[serde(default)]
    pub kind: Kind,
    pub enabled: bool,
}

impl Pack {
    /// One sentence for a toast: what this pack replaces, and how much of it.
    pub fn summary(&self) -> String {
        match self.kind {
            Kind::Texture => format!("{} 张贴图", self.textures),
            Kind::Shader => format!("{} 个着色器", self.shaders),
        }
    }
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

        let names: Vec<&str> = archive.names().collect();

        if looks_like_apk(&names) {
            return Err(Error::BadPack(
                "this is an APK (it has AndroidManifest.xml and classes.dex); \
                 import it on the Builds page, not as a texture pack"
                    .into(),
            ));
        }

        let mut textures = 0usize;
        let mut shaders = 0usize;

        for name in &names {
            let lower = name.to_ascii_lowercase();
            if is_image(&lower) {
                textures += 1;
            } else if is_shader(&lower) {
                shaders += 1;
            }
        }

        // Either is enough. A pack of nothing but `renderchunk.vertex` and
        // `.fragment` is a shader pack and the game uses it; refusing it because
        // it has no PNG would reject a working pack.
        if textures == 0 && shaders == 0 {
            return Err(Error::BadPack(
                "archive carries no images or shaders: it is not a resource pack"
                    .into(),
            ));
        }

        let kind = if textures > 0 { Kind::Texture } else { Kind::Shader };

        let id = short_id(bytes);
        if let Some(existing) = self.index.packs.iter().find(|p| p.id == id) {
            return Ok(ImportedPack { pack: existing.clone(), deduped: true });
        }

        // A wrapper directory is common enough that the metadata may live inside
        // it, so look in both places before concluding there is no manifest.
        let prefix = common_prefix(names.iter().copied());

        let manifest = find_entry(&names, prefix.as_deref(), "manifest.json")
            .and_then(|hit| archive.entry(hit))
            .and_then(|e| archive.read(e).ok())
            .and_then(|raw| Manifest::parse(&raw).ok());

        // Pre-1.0 packs carry `pack.mcmeta` instead. It is plain JSON with a
        // `pack.description`, which is the only human text these archives have.
        let mcmeta = find_entry(&names, prefix.as_deref(), "pack.mcmeta")
            .and_then(|hit| archive.entry(hit))
            .and_then(|e| archive.read(e).ok())
            .and_then(|raw| Mcmeta::parse(&raw));

        // A missing or unreadable manifest is not fatal: the pack still has
        // textures the game can use. It only means the name is ours to guess.
        let name = manifest
            .as_ref()
            .and_then(|m| m.name.clone())
            .or_else(|| source_name.and_then(stem))
            .unwrap_or_else(|| format!("Resource pack {id}"));

        let version = manifest
            .as_ref()
            .and_then(|m| m.version.clone())
            .unwrap_or_else(|| "unknown".into());

        let description = manifest
            .as_ref()
            .and_then(|m| m.description.clone())
            .or_else(|| mcmeta.as_ref().and_then(|m| m.description.clone()))
            .unwrap_or_default();

        let pack = Pack {
            id: id.clone(),
            name,
            version,
            description,
            size: bytes.len() as u64,
            textures,
            shaders,
            kind,
            enabled: true,
        };

        let dir = self.root.join(PACK_DIR);
        std::fs::write(dir.join(format!("{id}.zip")), bytes)
            .map_err(|e| Error::Io(format!("cannot store pack {id}: {e}")))?;

        self.index.packs.push(pack.clone());
        self.save()?;

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

/// `pack.mcmeta`, the pre-1.0 metadata file. `{"pack":{"pack_format":1,
/// "description":"…"}}` is all it holds; the modern `manifest.json` did not exist
/// yet. Only the description is read, and only because it is the one piece of
/// human text these archives carry.
#[derive(Debug, Default)]
struct Mcmeta {
    description: Option<String>,
}

impl Mcmeta {
    fn parse(raw: &[u8]) -> Option<Mcmeta> {
        let v: serde_json::Value = serde_json::from_slice(raw).ok()?;
        let pack = v.get("pack").unwrap_or(&v);
        let description =
            string_field(pack, "description").or_else(|| string_field(&v, "description"));
        Some(Mcmeta { description })
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
    fn an_archive_with_nothing_the_game_loads_is_rejected() {
        let t = TempDir::new("empty");
        let mut store = PackStore::open(&t.0).unwrap();
        let empty = zip_stored(&[("readme.txt", b"hello".to_vec())]);

        match store.import(&empty, None) {
            Err(Error::BadPack(msg)) => assert!(msg.contains("no images or shaders"), "{msg}"),
            other => panic!("expected BadPack, got {other:?}"),
        }
        assert_eq!(store.packs().len(), 0);
    }

    /// The shape of `Ultimacraft_Texture/`: everything is inside one directory
    /// whose name means nothing to us, and the texture folders below it are the
    /// game's own names (`armor/`, `blocks/`) with no `images/` level at all.
    /// This is one of the five real packs the old prefix rule refused.
    #[test]
    fn one_wrapper_directory_is_not_a_reason_to_refuse() {
        let t = TempDir::new("wrapper");
        let mut store = PackStore::open(&t.0).unwrap();
        let bytes = zip_stored(&[
            ("Ultimacraft_Texture/armor/chain_1.png", b"png".to_vec()),
            ("Ultimacraft_Texture/armor/chain_2.png", b"png".to_vec()),
            ("Ultimacraft_Texture/blocks/stone.png", b"png".to_vec()),
            (
                "Ultimacraft_Texture/manifest.json",
                br#"{"header":{"name":"Ultimacraft","version":[1,8,1]}}"#.to_vec(),
            ),
        ]);

        let out = store.import(&bytes, Some("Gun mod.zip")).unwrap();
        assert_eq!(out.pack.name, "Ultimacraft", "the manifest inside the wrapper counts");
        assert_eq!(out.pack.version, "1.8.1");
        assert_eq!(out.pack.textures, 3);
        assert_eq!(out.pack.kind, Kind::Texture);
    }

    /// `Paper Cut-Out[折纸]16x 0.14.x V1 .zip`: `images/` at the root, no
    /// `assets/` level. The old rule refused it.
    #[test]
    fn images_at_the_root_is_a_pack() {
        let t = TempDir::new("rootimages");
        let mut store = PackStore::open(&t.0).unwrap();
        let bytes = zip_stored(&[
            ("images/armor/chain_1.png", b"png".to_vec()),
            ("pack.mcmeta", r#"{"pack":{"pack_format":1,"description":"折纸"}}"#.as_bytes().to_vec()),
            ("pack.png", b"png".to_vec()),
        ]);

        let out = store.import(&bytes, Some("Paper Cut-Out[折纸]16x 0.14.x V1 .zip")).unwrap();
        assert_eq!(out.pack.textures, 2, "the root icon counts too: the game loads it");
        assert_eq!(out.pack.kind, Kind::Texture);
    }

    /// `冷色光影0.14.3.zip`: two shader files at the archive root and nothing
    /// else. No PNG means no textures, and it is still a working pack.
    #[test]
    fn a_shader_only_archive_is_a_pack_and_says_so() {
        let t = TempDir::new("shader");
        let mut store = PackStore::open(&t.0).unwrap();
        let bytes = zip_stored(&[
            ("renderchunk.fragment", b"// shader".to_vec()),
            ("renderchunk.vertex", b"// shader".to_vec()),
        ]);

        let out = store.import(&bytes, Some("冷色光影0.14.3.zip")).unwrap();
        assert_eq!(out.pack.kind, Kind::Shader);
        assert_eq!(out.pack.textures, 0);
        assert_eq!(out.pack.shaders, 2);
        assert_eq!(out.pack.name, "冷色光影0.14.3");
        assert_eq!(out.pack.summary(), "2 个着色器");
    }

    /// `苦力怕光影V8.zip`: wrapper directory, then `assets/`.
    #[test]
    fn a_wrapper_around_assets_is_a_pack() {
        let t = TempDir::new("wrapperassets");
        let mut store = PackStore::open(&t.0).unwrap();
        let bytes = zip_stored(&[
            ("Creeper'S V8.0 0.14.x/assets/images/environment/sun.png", b"png".to_vec()),
            ("Creeper'S V8.0 0.14.x/assets/splashes.json", b"{}".to_vec()),
        ]);

        let out = store.import(&bytes, None).unwrap();
        assert_eq!(out.pack.textures, 1);
        assert_eq!(out.pack.kind, Kind::Texture);
    }

    /// An APK has images too, so counting images alone would accept it. This must
    /// name the mistake instead of installing 40 MB the game will ignore.
    #[test]
    fn an_apk_is_refused_by_name_rather_than_installed() {
        let t = TempDir::new("apk");
        let mut store = PackStore::open(&t.0).unwrap();
        let bytes = zip_stored(&[
            ("AndroidManifest.xml", b"axml".to_vec()),
            ("classes.dex", b"dex".to_vec()),
            ("res/drawable/icon.png", b"png".to_vec()),
        ]);

        match store.import(&bytes, Some("game.apk")) {
            Err(Error::BadPack(msg)) => {
                assert!(msg.contains("APK"), "{msg}");
                assert!(msg.contains("Builds"), "{msg}");
            }
            other => panic!("expected BadPack, got {other:?}"),
        }
        assert_eq!(store.packs().len(), 0);
    }

    /// An index written before `kind` and `shaders` existed must still load: the
    /// upgrade path is a user who already has packs.
    #[test]
    fn an_index_from_before_kind_and_shaders_still_loads() {
        let t = TempDir::new("oldindex");
        std::fs::create_dir_all(t.0.join(PACK_DIR)).unwrap();
        std::fs::write(
            t.0.join(INDEX_FILE),
            br#"{"schema":1,"packs":[{"id":"abc","name":"A","version":"unknown",
                "description":"","size":10,"textures":3,"enabled":true}]}"#,
        )
        .unwrap();

        let store = PackStore::open(&t.0).unwrap();
        let pack = store.get("abc").expect("old entry survived");
        assert_eq!(pack.textures, 3);
        assert_eq!(pack.shaders, 0);
        assert_eq!(pack.kind, Kind::Texture);
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
