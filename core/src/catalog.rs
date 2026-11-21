//! The version library on disk: imported game builds plus their metadata.
//!
//! The store is a directory with a JSON index and content-addressed APK files.
//! Two properties matter more than speed here:
//!
//! * **Import is idempotent.** The identity of a build is its SHA-256, so
//!   importing the same APK twice returns the existing entry instead of a
//!   duplicate — a user who adds a build "again" gets a clear answer, not a
//!   library that grows every time they tap the button.
//! * **A half-written index is never observed.** The index is written to a
//!   temporary file and renamed over the old one, so a crash mid-save leaves the
//!   previous index intact rather than a truncated JSON file.

use crate::apk;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const SCHEMA: u32 = 1;
pub const INDEX_FILE: &str = "versions.json";
pub const APK_DIR: &str = "apks";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionEntry {
    /// Stable id derived from the content hash, so it survives a re-import.
    pub id: String,
    pub package: String,
    pub version_name: String,
    pub version_code: u64,
    pub min_sdk: u32,
    pub target_sdk: u32,
    pub abis: Vec<String>,
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher_activity: Option<String>,
    /// Unix seconds, so the list can be ordered by "most recently added"
    /// without depending on file mtimes, which Android likes to rewrite.
    pub imported_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Catalog {
    pub schema: u32,
    pub versions: Vec<VersionEntry>,
}

impl Default for Catalog {
    fn default() -> Self {
        Catalog {
            schema: SCHEMA,
            versions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    pub entry: VersionEntry,
    /// True when this exact APK was already in the library.
    pub deduped: bool,
}

#[derive(Debug)]
pub struct Library {
    root: PathBuf,
    catalog: Catalog,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let d = h.finalize();
    let mut out = String::with_capacity(64);
    for b in d {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Library {
    /// Open (or create) a library rooted at `root`.
    pub fn open(root: &Path) -> Result<Library> {
        std::fs::create_dir_all(root.join(APK_DIR))
            .map_err(|e| Error::Io(format!("cannot create {}: {e}", root.display())))?;

        let index = root.join(INDEX_FILE);
        let catalog = if index.exists() {
            let text = std::fs::read_to_string(&index)
                .map_err(|e| Error::Io(format!("cannot read {}: {e}", index.display())))?;
            let parsed: Catalog = serde_json::from_str(&text)
                .map_err(|e| Error::Corrupt(format!("{}: {e}", index.display())))?;
            if parsed.schema != SCHEMA {
                return Err(Error::Corrupt(format!(
                    "index schema {} is not supported (this build understands {SCHEMA})",
                    parsed.schema
                )));
            }
            parsed
        } else {
            Catalog::default()
        };

        Ok(Library {
            root: root.to_path_buf(),
            catalog,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn versions(&self) -> &[VersionEntry] {
        &self.catalog.versions
    }

    pub fn get(&self, id: &str) -> Option<&VersionEntry> {
        self.catalog.versions.iter().find(|v| v.id == id)
    }

    pub fn apk_path(&self, sha256: &str) -> PathBuf {
        self.root.join(APK_DIR).join(format!("{sha256}.apk"))
    }

    /// Import an APK held in memory.
    pub fn import(&mut self, bytes: &[u8]) -> Result<ImportOutcome> {
        if bytes.is_empty() {
            return Err(Error::NotZip("file is empty".into()));
        }
        let digest = sha256_hex(bytes);

        if let Some(existing) = self
            .catalog
            .versions
            .iter()
            .find(|v| v.sha256 == digest)
            .cloned()
        {
            return Ok(ImportOutcome {
                entry: existing,
                deduped: true,
            });
        }

        let info = apk::inspect(bytes)?;
        let entry = VersionEntry {
            id: format!("mc-{}", &digest[..12]),
            package: info.manifest.package.clone(),
            version_name: info.manifest.version_name.clone(),
            version_code: info.manifest.version_code,
            min_sdk: info.manifest.min_sdk,
            target_sdk: info.manifest.target_sdk,
            abis: info.abis.clone(),
            sha256: digest.clone(),
            size: bytes.len() as u64,
            label: info.manifest.label.clone(),
            launcher_activity: info.manifest.launcher_activity.clone(),
            imported_at: now_secs(),
        };

        // Copy the payload before touching the index: an index entry pointing at
        // a file that failed to land is worse than no entry at all.
        let dest = self.apk_path(&digest);
        write_atomic(&dest, bytes)?;

        self.catalog.versions.push(entry.clone());
        self.save()?;
        Ok(ImportOutcome {
            entry,
            deduped: false,
        })
    }

    /// Remove one build, deleting its APK only when nothing else refers to it.
    pub fn remove(&mut self, id: &str) -> Result<VersionEntry> {
        let pos = self
            .catalog
            .versions
            .iter()
            .position(|v| v.id == id)
            .ok_or_else(|| Error::NotFound(format!("version '{id}'")))?;
        let removed = self.catalog.versions.remove(pos);

        if !self.catalog.versions.iter().any(|v| v.sha256 == removed.sha256) {
            let p = self.apk_path(&removed.sha256);
            if p.exists() {
                // A missing payload is not a failure to remove the record.
                let _ = std::fs::remove_file(p);
            }
        }
        self.save()?;
        Ok(removed)
    }

    /// Persist the index atomically.
    pub fn save(&self) -> Result<()> {
        let index = self.root.join(INDEX_FILE);
        let tmp = self.root.join(format!("{INDEX_FILE}.tmp"));
        let text = serde_json::to_string_pretty(&self.catalog)
            .map_err(|e| Error::Corrupt(format!("cannot serialise index: {e}")))?;
        std::fs::write(&tmp, text.as_bytes())
            .map_err(|e| Error::Io(format!("cannot write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &index)
            .map_err(|e| Error::Io(format!("cannot replace {}: {e}", index.display())))?;
        Ok(())
    }
}

fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = dest.with_extension("apk.part");
    std::fs::write(&tmp, bytes)
        .map_err(|e| Error::Io(format!("cannot write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, dest)
        .map_err(|e| Error::Io(format!("cannot move into {}: {e}", dest.display())))?;
    Ok(())
}
