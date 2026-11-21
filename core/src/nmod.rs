//! Native mods ("nmod"): the `.so` payloads BlockLauncher loads into the game
//! process.
//!
//! A native mod is only dangerous in one specific way — it runs inside the game
//! with the game's privileges — so the job here is not sandboxing (that is
//! impossible for an injected library) but *refusing ambiguity*: a package that
//! cannot say which ABI it was built for, or which game versions it targets, is
//! rejected before anything is loaded. An ambiguous mod fails at `dlopen` with a
//! message the user cannot act on; a rejected one fails here with a message they
//! can.

use crate::error::{Error, Result};
use crate::version::{GameRange, GameVersion};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCHEMA: u32 = 1;
pub const MANIFEST_FILE: &str = "nmod.json";
pub const KNOWN_ABIS: [&str; 4] = ["arm64-v8a", "armeabi-v7a", "x86_64", "x86"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NmodManifest {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    /// File name of the library, e.g. `libmymod.so`. Never a path: the loader
    /// composes it with the ABI directory itself so a manifest cannot escape.
    pub entry: String,
    /// ABIs this package ships. Must be a subset of `KNOWN_ABIS`.
    pub abis: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_game: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_game: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn check_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 64 {
        return Err(Error::BadMod(format!(
            "id must be 1-64 characters, got {}",
            id.len()
        )));
    }
    let ok = id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.');
    if !ok {
        return Err(Error::BadMod(format!(
            "id '{id}' must be lowercase letters, digits, '-', '_' or '.'"
        )));
    }
    if id.starts_with('.') || id.ends_with('.') {
        return Err(Error::BadMod(format!(
            "id '{id}' must not start or end with '.'"
        )));
    }
    Ok(())
}

fn check_entry(entry: &str) -> Result<()> {
    if entry.is_empty() || entry.len() > 128 {
        return Err(Error::BadMod("entry must be 1-128 characters".into()));
    }
    if entry.contains('/') || entry.contains('\\') || entry.contains("..") {
        return Err(Error::BadMod(format!(
            "entry '{entry}' must be a bare file name, not a path"
        )));
    }
    if !entry.starts_with("lib") || !entry.ends_with(".so") {
        return Err(Error::BadMod(format!(
            "entry '{entry}' must look like lib<name>.so"
        )));
    }
    if entry.len() <= 5 {
        return Err(Error::BadMod(format!("entry '{entry}' has no name")));
    }
    Ok(())
}

impl NmodManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA {
            return Err(Error::BadMod(format!(
                "schema {} is not supported (this build understands {SCHEMA})",
                self.schema
            )));
        }
        check_id(&self.id)?;
        if self.name.trim().is_empty() || self.name.len() > 96 {
            return Err(Error::BadMod("name must be 1-96 characters".into()));
        }
        if self.version.trim().is_empty() || self.version.len() > 32 {
            return Err(Error::BadMod("version must be 1-32 characters".into()));
        }
        check_entry(&self.entry)?;

        if self.abis.is_empty() {
            return Err(Error::BadMod(
                "abis is empty: a native mod that names no ABI cannot be loaded safely".into(),
            ));
        }
        let mut seen: Vec<&str> = Vec::new();
        for abi in &self.abis {
            if !KNOWN_ABIS.contains(&abi.as_str()) {
                return Err(Error::BadMod(format!(
                    "abi '{abi}' is not one of {}",
                    KNOWN_ABIS.join(", ")
                )));
            }
            if seen.contains(&abi.as_str()) {
                return Err(Error::BadMod(format!("abi '{abi}' is listed twice")));
            }
            seen.push(abi.as_str());
        }

        // Building the range validates the two version strings.
        let range = self.range()?;
        if let (Some(min), Some(max)) = (&range.min, &range.max) {
            if min.compare(max) == std::cmp::Ordering::Greater {
                return Err(Error::BadMod(format!(
                    "min_game {} is greater than max_game {}",
                    min, max
                )));
            }
        }
        Ok(())
    }

    pub fn range(&self) -> Result<GameRange> {
        let parse = |v: &Option<String>, field: &str| -> Result<Option<GameVersion>> {
            match v {
                None => Ok(None),
                Some(s) => GameVersion::parse(s)
                    .map(Some)
                    .ok_or_else(|| Error::BadMod(format!("{field} '{s}' is not a version"))),
            }
        };
        Ok(GameRange {
            min: parse(&self.min_game, "min_game")?,
            max: parse(&self.max_game, "max_game")?,
        })
    }

    pub fn supports_abi(&self, abi: &str) -> bool {
        self.abis.iter().any(|a| a == abi)
    }

    pub fn supports_game(&self, v: &GameVersion) -> bool {
        match self.range() {
            Ok(r) => r.contains(v),
            // Unreachable after `validate`, but treating a bad range as
            // "matches nothing" is the safe direction for a loader.
            Err(_) => false,
        }
    }
}

/// A native mod that has been read off disk and checked against what is
/// actually present in its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nmod {
    pub manifest: NmodManifest,
    pub root: PathBuf,
}

impl Nmod {
    /// Path of the library for one ABI, or `None` when the package does not
    /// cover that ABI.
    pub fn lib_path(&self, abi: &str) -> Option<PathBuf> {
        if !self.manifest.supports_abi(abi) {
            return None;
        }
        Some(
            self.root
                .join("lib")
                .join(abi)
                .join(&self.manifest.entry),
        )
    }

    /// Read `<root>/nmod.json` and verify every declared ABI has its library.
    ///
    /// A manifest that promises `arm64-v8a` and ships only `armeabi-v7a` is a
    /// packaging mistake that would surface as a `dlopen` failure at game start;
    /// catching it at install time is the whole reason this checks the disk.
    pub fn load(root: &Path) -> Result<Nmod> {
        let path = root.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            Error::BadMod(format!("cannot read {}: {e}", path.display()))
        })?;
        let manifest: NmodManifest = serde_json::from_str(&text)
            .map_err(|e| Error::BadMod(format!("{} is not valid JSON: {e}", path.display())))?;
        manifest.validate()?;

        for abi in &manifest.abis {
            let lib = root.join("lib").join(abi).join(&manifest.entry);
            if !lib.is_file() {
                return Err(Error::BadMod(format!(
                    "manifest claims {abi} but {} is missing",
                    lib.display()
                )));
            }
        }
        Ok(Nmod {
            manifest,
            root: root.to_path_buf(),
        })
    }
}
