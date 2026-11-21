//! Native mods — the "nmod" package format of the EnderCore/ModdedMCPE
//! launcher, which is what BlockLauncher's native mods became.
//!
//! An nmod is **a zip with `nmod_manifest.json` at its root**. Inside, each game
//! version family gets its own directory under `android/`:
//!
//! ```text
//! nmod_manifest.json
//! android/<support-name>/assets/<file>        overrides applied to the game's assets
//! android/<support-name>/<abi>/lib<name>.so   libraries loaded into the game
//! icon.png, banner.png                        referenced by the manifest
//! ```
//!
//! Two details are easy to get wrong and are the reason this module exists
//! rather than a `serde` derive on a struct:
//!
//! * `target_game_versions` holds **regular expressions**, matched against the
//!   game's version *name* with full-match semantics (`Pattern.matches`), not
//!   version ranges. `0\.14\..*` is a version family; `*` is a syntax error.
//! * an override's payload is *referenced by path* and must exist in the zip at
//!   import time. A package that names a file it does not ship is rejected when
//!   the user can still do something about it, not at game start.
//!
//! The field names, the required/optional split and the ordering follow
//! `org.endercore.android.mod.nmod.NModPackage` from ModdedMCPE-Android. Where
//! this module is stricter than the reference it says so at the check.

use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::apk::{Apk, Entry};

/// Manifest file name at the root of the package. Not configurable: it is how
/// the reference recognises an nmod at all.
pub const MANIFEST: &str = "nmod_manifest.json";

/// The only platform this build ships. Other platforms in `game_supports` are
/// skipped rather than rejected, exactly as the reference does.
pub const PLATFORM: &str = "android";

/// Mirrors `EnderCore.SDK_INT`. The reference refuses a package whose
/// `min_sdk_version` is higher than the runtime it was built into; keeping the
/// same number means the same packages load here.
pub const SDK_INT: i32 = 1;

/// A cap on the manifest, so a hostile zip cannot make us allocate a gigabyte by
/// declaring a huge uncompressed size for the manifest entry.
const MAX_MANIFEST_BYTES: usize = 512 * 1024;

/// A cap on one override payload. Assets are `assets/` files from a mobile game;
/// anything past this is not a texture or a JSON table.
const MAX_OVERRIDE_BYTES: usize = 64 * 1024 * 1024;

// ---------------------------------------------------------------- manifest

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub uuid: Option<String>,
    pub name: Option<String>,
    pub version_name: Option<String>,
    pub version_code: Option<i32>,
    pub manifest_version: Option<i32>,
    pub min_sdk_version: Option<i32>,
    pub target_sdk_version: Option<i32>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub author_email: Option<String>,
    pub change_log: Option<String>,
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub game_supports: Option<Vec<GameSupport>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameSupport {
    pub name: Option<String>,
    pub platform: Option<String>,
    /// Regular expressions, full-matched against the game's version name.
    pub target_game_versions: Option<Vec<String>>,
    pub native_libs: Option<Vec<NativeLib>>,
    pub text_overrides: Option<Vec<TextOverride>>,
    pub json_overrides: Option<Vec<JsonOverride>>,
    pub file_overrides: Option<Vec<FileOverride>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NativeLib {
    pub name: Option<String>,
    /// Entry point, as the reference's manifest schema allows. The launcher does
    /// not use it (it calls `System.loadLibrary(name)`), so it is carried, not
    /// interpreted.
    pub main: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextOverride {
    pub path: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonOverride {
    pub path: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileOverride {
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextMode {
    Replace,
    Append,
    Prepend,
}

impl TextMode {
    fn parse(s: &str) -> Option<TextMode> {
        match s {
            "replace" => Some(TextMode::Replace),
            "append" => Some(TextMode::Append),
            "prepend" => Some(TextMode::Prepend),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TextMode::Replace => "replace",
            TextMode::Append => "append",
            TextMode::Prepend => "prepend",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonMode {
    Replace,
    Merge,
}

impl JsonMode {
    fn parse(s: &str) -> Option<JsonMode> {
        match s {
            "replace" => Some(JsonMode::Replace),
            "merge" => Some(JsonMode::Merge),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            JsonMode::Replace => "replace",
            JsonMode::Merge => "merge",
        }
    }
}

// ------------------------------------------------------------------ package

/// An nmod zip, read and validated.
///
/// Holds the archive bytes because every override payload is read lazily by
/// path; keeping the buffer means one parse instead of one file handle.
pub struct Package {
    buf: Vec<u8>,
    manifest: Manifest,
    warnings: Vec<String>,
    entries: Vec<Entry>,
}

impl std::fmt::Debug for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("nmod::Package")
            .field("uuid", &self.uuid())
            .field("name", &self.name())
            .field("supports", &self.support_names())
            .finish()
    }
}

/// A path inside `assets/` that an override writes to.
///
/// The reference joins the manifest's string straight onto the assets directory,
/// which lets `../../` write outside it. A package is data from the internet, so
/// this refuses anything that is not a plain relative path — the one place this
/// module is deliberately stricter than the reference.
fn safe_relative(path: &str) -> Result<PathBuf> {
    if path.is_empty() {
        return Err(Error::BadMod("override path is empty".into()));
    }
    if path.len() > 256 {
        return Err(Error::BadMod(format!(
            "override path is {} characters; 256 is the limit",
            path.len()
        )));
    }
    if path.contains('\0') {
        return Err(Error::BadMod("override path contains a NUL".into()));
    }
    if path.contains('\\') {
        return Err(Error::BadMod(format!(
            "override path '{path}' uses a backslash; nmod paths are '/'-separated"
        )));
    }
    if path.starts_with('/') {
        return Err(Error::BadMod(format!(
            "override path '{path}' is absolute; it must be relative to assets/"
        )));
    }
    let p = Path::new(path);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(Error::BadMod(format!(
                    "override path '{path}' contains '..', which would leave assets/"
                )))
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(Error::BadMod(format!(
                    "override path '{path}' is absolute"
                )))
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(Error::BadMod(format!("override path '{path}' names no file")));
    }
    Ok(out)
}

/// Full-match a Java-style `Pattern.matches` against `version`.
///
/// `regex-lite` is used rather than `regex` because this runs on Android where
/// every kilobyte is downloaded twice (two ABIs) and version patterns are ASCII
/// by construction.
fn pattern_matches(pattern: &str, version: &str) -> Result<bool> {
    let anchored = format!("^(?:{})$", pattern);
    let re = regex_lite::Regex::new(&anchored).map_err(|e| {
        Error::BadMod(format!(
            "target_game_versions entry '{pattern}' is not a valid pattern: {e}"
        ))
    })?;
    Ok(re.is_match(version))
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, c) in b.iter().enumerate() {
        let dash = matches!(i, 8 | 13 | 18 | 23);
        if dash {
            if *c != b'-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

impl Package {
    /// Read and validate an nmod package.
    ///
    /// Errors name the manifest field that is wrong, because the person who has
    /// to fix it is the person who packed it.
    pub fn open(bytes: Vec<u8>) -> Result<Package> {
        let manifest = Self::read_manifest(&bytes)?;
        let mut warnings: Vec<String> = Vec::new();

        let uuid = manifest
            .uuid
            .as_deref()
            .ok_or_else(|| Error::BadMod("manifest has no uuid".into()))?;
        if !is_uuid(uuid) {
            return Err(Error::BadMod(format!(
                "uuid '{uuid}' is not a UUID (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)"
            )));
        }

        let min_sdk = manifest
            .min_sdk_version
            .ok_or_else(|| Error::BadMod("manifest has no min_sdk_version".into()))?;
        if min_sdk > SDK_INT {
            return Err(Error::BadMod(format!(
                "min_sdk_version {min_sdk} is above this runtime's {SDK_INT}; the package needs a newer launcher"
            )));
        }
        if manifest.target_sdk_version.is_none() {
            return Err(Error::BadMod("manifest has no target_sdk_version".into()));
        }
        if manifest.manifest_version.is_none() {
            return Err(Error::BadMod("manifest has no manifest_version".into()));
        }
        let name = manifest
            .name
            .as_deref()
            .ok_or_else(|| Error::BadMod("manifest has no name".into()))?;
        if name.trim().is_empty() {
            return Err(Error::BadMod("manifest name is empty".into()));
        }
        if manifest.version_name.is_none() {
            return Err(Error::BadMod("manifest has no version_name".into()));
        }
        if manifest.version_code.is_none() {
            return Err(Error::BadMod(
                "manifest has no version_code (needed to tell a newer package from an older one)".into(),
            ));
        }

        for (field, value) in [
            ("author", &manifest.author),
            ("author_email", &manifest.author_email),
            ("icon", &manifest.icon),
            ("banner", &manifest.banner),
        ] {
            if value.is_none() {
                warnings.push(format!("manifest has no {field}"));
            }
        }

        let supports = manifest
            .game_supports
            .as_deref()
            .ok_or_else(|| Error::BadMod("manifest has no game_supports".into()))?;
        if supports.is_empty() {
            warnings.push("game_supports is empty: nothing can ever enable this mod".into());
        }

        let zip = Apk::open(&bytes)?;

        for (i, support) in supports.iter().enumerate() {
            Self::check_support(&zip, i, support)?;
        }

        for (field, declared) in [("icon", &manifest.icon), ("banner", &manifest.banner)] {
            if let Some(path) = declared {
                if zip.entry(path).is_none() {
                    return Err(Error::BadMod(format!(
                        "manifest {field} points at '{path}', which is not in the package"
                    )));
                }
            }
        }

        // `Apk` borrows the buffer and so cannot be stored beside it; the entry
        // list is copied out instead of re-parsing on every lookup.
        let entries = Apk::open(&bytes)?.entries().to_vec();

        Ok(Package {
            buf: bytes,
            manifest,
            warnings,
            entries,
        })
    }

    fn read_manifest(bytes: &[u8]) -> Result<Manifest> {
        let zip = Apk::open(bytes)?;
        let entry = zip
            .entry(MANIFEST)
            .ok_or_else(|| {
                Error::BadMod(format!(
                    "no {MANIFEST} at the root; an nmod is a zip whose manifest is at the top level"
                ))
            })?
            .clone();
        if entry.uncompressed_size as usize > MAX_MANIFEST_BYTES {
            return Err(Error::BadMod(format!(
                "{} declares {} bytes; the limit is {MAX_MANIFEST_BYTES}",
                MANIFEST, entry.uncompressed_size
            )));
        }
        let raw = zip.read(&entry)?;
        let text = String::from_utf8(raw)
            .map_err(|_| Error::BadMod(format!("{MANIFEST} is not UTF-8")))?;
        serde_json::from_str(&text)
            .map_err(|e| Error::BadMod(format!("{MANIFEST} is not valid JSON: {e}")))
    }

    fn check_support(zip: &Apk<'_>, index: usize, support: &GameSupport) -> Result<()> {
        let ord = index + 1;
        let platform = support
            .platform
            .as_deref()
            .ok_or_else(|| Error::BadMod(format!("game_support #{ord} has no platform")))?;
        if platform != PLATFORM {
            // Not an error: a package may support several platforms and this
            // build only implements one.
            return Ok(());
        }
        let name = support
            .name
            .as_deref()
            .ok_or_else(|| Error::BadMod(format!("game_support #{ord} has no name")))?;
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(Error::BadMod(format!(
                "game_support #{ord} name '{name}' must be one directory name"
            )));
        }

        let versions = support.target_game_versions.as_deref().ok_or_else(|| {
            Error::BadMod(format!(
                "game_support #{ord} has no target_game_versions"
            ))
        })?;
        if versions.is_empty() {
            return Err(Error::BadMod(format!(
                "game_support #{ord} lists no target_game_versions"
            )));
        }
        for pattern in versions {
            if pattern.is_empty() {
                return Err(Error::BadMod(format!(
                    "game_support #{ord} has an empty target_game_versions entry"
                )));
            }
            // Compile now so a typo is reported at import, not at launch.
            pattern_matches(pattern, "0")?;
        }

        if let Some(libs) = support.native_libs.as_deref() {
            for lib in libs {
                let lib_name = lib.name.as_deref().ok_or_else(|| {
                    Error::BadMod(format!("game_support #{ord} has a native lib with no name"))
                })?;
                if lib_name.is_empty() {
                    return Err(Error::BadMod(format!(
                        "game_support #{ord} has a native lib with an empty name"
                    )));
                }
            }
        }

        if let Some(list) = support.text_overrides.as_deref() {
            for o in list {
                let path = o.path.as_deref().ok_or_else(|| {
                    Error::BadMod(format!("game_support #{ord} has a text override with no path"))
                })?;
                safe_relative(path)?;
                let mode = o.mode.as_deref().unwrap_or("replace");
                if TextMode::parse(mode).is_none() {
                    return Err(Error::BadMod(format!(
                        "game_support #{ord} text override '{path}' has mode '{mode}'; expected replace, append or prepend"
                    )));
                }
                Self::require_asset(zip, name, path, "text override", ord)?;
            }
        }

        if let Some(list) = support.json_overrides.as_deref() {
            for o in list {
                let path = o.path.as_deref().ok_or_else(|| {
                    Error::BadMod(format!("game_support #{ord} has a json override with no path"))
                })?;
                safe_relative(path)?;
                let mode = o.mode.as_deref().unwrap_or("replace");
                if JsonMode::parse(mode).is_none() {
                    return Err(Error::BadMod(format!(
                        "game_support #{ord} json override '{path}' has mode '{mode}'; expected replace or merge"
                    )));
                }
                Self::require_asset(zip, name, path, "json override", ord)?;
            }
        }

        if let Some(list) = support.file_overrides.as_deref() {
            for o in list {
                let path = o.path.as_deref().ok_or_else(|| {
                    Error::BadMod(format!("game_support #{ord} has a file override with no path"))
                })?;
                safe_relative(path)?;
                Self::require_asset(zip, name, path, "file override", ord)?;
            }
        }

        Ok(())
    }

    fn require_asset(
        zip: &Apk<'_>,
        support_name: &str,
        path: &str,
        what: &str,
        ord: usize,
    ) -> Result<()> {
        let in_zip = format!("{PLATFORM}/{support_name}/assets/{path}");
        if zip.entry(&in_zip).is_none() {
            return Err(Error::BadMod(format!(
                "game_support #{ord} {what} '{path}' is not in the package (looked for '{in_zip}')"
            )));
        }
        Ok(())
    }

    // ------------------------------------------------------------- accessors

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn uuid(&self) -> &str {
        self.manifest.uuid.as_deref().unwrap_or("")
    }

    pub fn name(&self) -> &str {
        self.manifest.name.as_deref().unwrap_or("")
    }

    pub fn version_name(&self) -> &str {
        self.manifest.version_name.as_deref().unwrap_or("")
    }

    pub fn version_code(&self) -> i32 {
        self.manifest.version_code.unwrap_or(0)
    }

    pub fn author(&self) -> &str {
        self.manifest.author.as_deref().unwrap_or("")
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buf
    }

    /// Names of the `android/` game supports, in manifest order.
    pub fn support_names(&self) -> Vec<&str> {
        self.manifest
            .game_supports
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter(|s| s.platform.as_deref() == Some(PLATFORM))
            .filter_map(|s| s.name.as_deref())
            .collect()
    }

    /// The first game support whose `target_game_versions` matches `version`.
    ///
    /// Matches the reference: manifest order decides, and the match is a
    /// full-match regex, not a substring.
    pub fn support_for(&self, version: &str) -> Result<Option<&GameSupport>> {
        let supports = self.manifest.game_supports.as_deref().unwrap_or(&[]);
        for support in supports {
            if support.platform.as_deref() != Some(PLATFORM) {
                continue;
            }
            for pattern in support.target_game_versions.as_deref().unwrap_or(&[]) {
                if pattern_matches(pattern, version)? {
                    return Ok(Some(support));
                }
            }
        }
        Ok(None)
    }

    /// Library file names for one support and ABI, as they appear in the zip.
    ///
    /// `System.loadLibrary("foo")` resolves `libfoo.so`, so a manifest name of
    /// `foo` is looked up as `libfoo.so` first and as the literal name second —
    /// some packs ship the `lib` prefix in the name already.
    pub fn native_libs(&self, support: &GameSupport, abi: &str) -> Vec<String> {
        let name = support.name.as_deref().unwrap_or("");
        support
            .native_libs
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|l| l.name.as_deref())
            .filter_map(|n| {
                let with_prefix = format!("{PLATFORM}/{name}/{abi}/lib{n}.so");
                let literal = format!("{PLATFORM}/{name}/{abi}/{n}");
                if self.entries.iter().any(|e| e.name == with_prefix) {
                    Some(with_prefix)
                } else if self.entries.iter().any(|e| e.name == literal) {
                    Some(literal)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Every ABI directory this package ships for one support.
    pub fn shipped_abis(&self, support: &GameSupport) -> Vec<String> {
        let name = support.name.as_deref().unwrap_or("");
        let prefix = format!("{PLATFORM}/{name}/");
        let mut set: BTreeSet<String> = BTreeSet::new();
        for e in &self.entries {
            let Some(rest) = e.name.strip_prefix(&prefix) else {
                continue;
            };
            let Some((abi, _)) = rest.split_once('/') else {
                continue;
            };
            if abi == "assets" || abi.is_empty() {
                continue;
            }
            set.insert(abi.to_owned());
        }
        set.into_iter().collect()
    }

    fn read_entry(&self, name: &str) -> Result<Vec<u8>> {
        let zip = Apk::open(&self.buf)?;
        let entry = zip
            .entry(name)
            .ok_or_else(|| Error::BadMod(format!("'{name}' is not in the package")))?;
        if entry.uncompressed_size as usize > MAX_OVERRIDE_BYTES {
            return Err(Error::BadMod(format!(
                "'{name}' declares {} bytes; the limit is {MAX_OVERRIDE_BYTES}",
                entry.uncompressed_size
            )));
        }
        zip.read(entry)
    }

    /// Read the manifest's `icon` bytes, if it declares one.
    pub fn icon(&self) -> Option<Vec<u8>> {
        self.manifest
            .icon
            .as_deref()
            .and_then(|p| self.read_entry(p).ok())
    }

    /// Read the manifest's `banner` bytes, if it declares one.
    pub fn banner(&self) -> Option<Vec<u8>> {
        self.manifest
            .banner
            .as_deref()
            .and_then(|p| self.read_entry(p).ok())
    }
}

// ----------------------------------------------------------------- overrides

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub path: String,
    pub kind: &'static str,
    pub mode: String,
    pub bytes: usize,
}

/// Apply one support's overrides onto an extracted `assets/` tree.
///
/// Order is the reference's: file, then json, then text. It matters when two
/// entries name the same path — a `file` copy followed by a `text` append is a
/// real pattern, and swapping them changes the result.
///
/// Returns the applied operations in order. An error stops the remaining
/// overrides for this support and leaves what was already written: the reference
/// behaves the same way (it catches per mod and logs), and a half-applied mod is
/// still better than a launcher that refuses to start the game.
pub fn apply(package: &Package, support: &GameSupport, assets_root: &Path) -> Result<Vec<Applied>> {
    let name = support.name.as_deref().unwrap_or("");
    let mut done = Vec::new();

    for o in support.file_overrides.as_deref().unwrap_or(&[]) {
        let rel = safe_relative(o.path.as_deref().unwrap_or(""))?;
        let src = format!("{PLATFORM}/{name}/assets/{}", rel.to_string_lossy());
        let data = package.read_entry(&src)?;
        write_file(&assets_root.join(&rel), &data)?;
        done.push(Applied {
            path: rel.to_string_lossy().into_owned(),
            kind: "file",
            mode: "replace".into(),
            bytes: data.len(),
        });
    }

    for o in support.json_overrides.as_deref().unwrap_or(&[]) {
        let rel = safe_relative(o.path.as_deref().unwrap_or(""))?;
        let mode = JsonMode::parse(o.mode.as_deref().unwrap_or("replace"))
            .ok_or_else(|| Error::BadMod("invalid json override mode".into()))?;
        let src = format!("{PLATFORM}/{name}/assets/{}", rel.to_string_lossy());
        let data = package.read_entry(&src)?;
        let dest = assets_root.join(&rel);
        match mode {
            JsonMode::Replace => {
                write_file(&dest, &data)?;
                done.push(Applied {
                    path: rel.to_string_lossy().into_owned(),
                    kind: "json",
                    mode: mode.as_str().into(),
                    bytes: data.len(),
                });
            }
            JsonMode::Merge => {
                let existing = std::fs::read(&dest).map_err(|e| {
                    Error::BadMod(format!(
                        "json override '{}' merges into a file the game does not have: {e}",
                        rel.display()
                    ))
                })?;
                let merged = merge_json(&data, &existing).map_err(|e| {
                    Error::BadMod(format!("json override '{}': {e}", rel.display()))
                })?;
                write_file(&dest, &merged)?;
                done.push(Applied {
                    path: rel.to_string_lossy().into_owned(),
                    kind: "json",
                    mode: mode.as_str().into(),
                    bytes: merged.len(),
                });
            }
        }
    }

    for o in support.text_overrides.as_deref().unwrap_or(&[]) {
        let rel = safe_relative(o.path.as_deref().unwrap_or(""))?;
        let mode = TextMode::parse(o.mode.as_deref().unwrap_or("replace"))
            .ok_or_else(|| Error::BadMod("invalid text override mode".into()))?;
        let src = format!("{PLATFORM}/{name}/assets/{}", rel.to_string_lossy());
        let data = package.read_entry(&src)?;
        let dest = assets_root.join(&rel);
        let written = match mode {
            TextMode::Replace => {
                write_file(&dest, &data)?;
                data.len()
            }
            TextMode::Append => {
                let mut existing = std::fs::read(&dest).map_err(|e| {
                    Error::BadMod(format!(
                        "text override '{}' appends to a missing file: {e}",
                        rel.display()
                    ))
                })?;
                existing.extend_from_slice(&data);
                write_file(&dest, &existing)?;
                existing.len()
            }
            TextMode::Prepend => {
                let existing = std::fs::read(&dest).map_err(|e| {
                    Error::BadMod(format!(
                        "text override '{}' prepends to a missing file: {e}",
                        rel.display()
                    ))
                })?;
                let mut out = data.clone();
                out.extend_from_slice(&existing);
                write_file(&dest, &out)?;
                out.len()
            }
        };
        done.push(Applied {
            path: rel.to_string_lossy().into_owned(),
            kind: "text",
            mode: mode.as_str().into(),
            bytes: written,
        });
    }

    Ok(done)
}

fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::BadMod(format!("cannot create {}: {e}", parent.display())))?;
    }
    std::fs::write(path, data)
        .map_err(|e| Error::BadMod(format!("cannot write {}: {e}", path.display())))
}

/// The reference's `JSONMerger`.
///
/// The semantics are worth stating plainly because they are surprising: the
/// **game's** values win on scalar conflicts, and the **game's** array elements
/// are appended after the mod's. A merge therefore adds keys the game does not
/// have and extends arrays; it does not overwrite anything the game already
/// defines. That is what `JsonOverrider` does, inverted-looking or not, and a
/// launcher that merged the other way would silently change what existing packs
/// produce.
pub fn merge_json(mod_side: &[u8], game_side: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let a: serde_json::Value =
        serde_json::from_slice(mod_side).map_err(|e| format!("the package's json is invalid: {e}"))?;
    let b: serde_json::Value =
        serde_json::from_slice(game_side).map_err(|e| format!("the game's json is invalid: {e}"))?;

    let merged = match (&a, &b) {
        (serde_json::Value::Object(_), serde_json::Value::Object(_)) => merge_value(a, b)?,
        (serde_json::Value::Array(_), serde_json::Value::Array(_)) => {
            // Arrays at the root concatenate, mod first.
            let (mut a, b) = match (a, b) {
                (serde_json::Value::Array(a), serde_json::Value::Array(b)) => (a, b),
                _ => unreachable!(),
            };
            a.extend(b);
            serde_json::Value::Array(a)
        }
        _ => return Err("cannot merge a json array with a json object".into()),
    };
    serde_json::to_vec(&merged).map_err(|e| format!("cannot re-encode the merged json: {e}"))
}

fn merge_value(a: serde_json::Value, b: serde_json::Value) -> std::result::Result<serde_json::Value, String> {
    let serde_json::Value::Object(mut a_map) = a else {
        return Err("only two objects merge key by key".into());
    };
    let serde_json::Value::Object(b_map) = b else {
        return Err("only two objects merge key by key".into());
    };
    for (key, b_val) in b_map {
        match (a_map.remove(&key), b_val) {
            (Some(serde_json::Value::Object(a_obj)), b_val @ serde_json::Value::Object(_)) => {
                a_map.insert(key, merge_value(serde_json::Value::Object(a_obj), b_val)?);
            }
            (Some(serde_json::Value::Array(mut a_arr)), serde_json::Value::Array(b_arr)) => {
                a_arr.extend(b_arr);
                a_map.insert(key, serde_json::Value::Array(a_arr));
            }
            (Some(_), serde_json::Value::Object(_))
            | (Some(_), serde_json::Value::Array(_)) => {
                return Err(format!(
                    "key '{key}' is an object/array on one side and not on the other"
                ));
            }
            (_, other) => {
                // Scalars: the game's value replaces the mod's, which is what
                // the reference's merge does.
                a_map.insert(key, other);
            }
        }
    }
    Ok(serde_json::Value::Object(a_map))
}

// --------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a zip with the given entries, deflated like a real pack.
    fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut central: Vec<u8> = Vec::new();
        let mut count = 0u16;

        for (name, data) in entries {
            let payload = miniz_oxide::deflate::compress_to_vec(data, 6);
            let offset = out.len() as u32;
            let crc = crc32(data);

            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&8u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&payload);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&8u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());

            count += 1;
        }

        let cd_offset = out.len() as u32;
        let cd_size = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
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

    const UUID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    fn manifest(extra: &str) -> String {
        // The extra members are spliced in only when present; an empty one would
        // leave a trailing comma, which is not JSON.
        let tail = if extra.is_empty() {
            String::new()
        } else {
            format!(",{extra}")
        };
        format!(
            r#"{{
                "uuid": "{UUID}",
                "name": "Test Pack",
                "version_name": "1.0",
                "version_code": 1,
                "manifest_version": 1,
                "min_sdk_version": 1,
                "target_sdk_version": 1,
                "author": "someone",
                "author_email": "a@b.c",
                "game_supports": [
                    {{
                        "platform": "android",
                        "name": "pe",
                        "target_game_versions": ["0\\.14\\..*"]
                        {tail}
                    }}
                ]
            }}"#
        )
    }

    fn pack(extra: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let m = manifest(extra);
        let mut all: Vec<(&str, &[u8])> = vec![(MANIFEST, m.as_bytes())];
        all.extend_from_slice(files);
        zip(&all)
    }

    #[test]
    fn a_minimal_package_opens() {
        let p = Package::open(pack("", &[])).unwrap();
        assert_eq!(p.uuid(), UUID);
        assert_eq!(p.name(), "Test Pack");
        assert_eq!(p.support_names(), vec!["pe"]);
        // author/email supplied, but icon and banner are not.
        assert_eq!(p.warnings().len(), 2, "{:?}", p.warnings());
    }

    #[test]
    fn a_missing_manifest_is_named() {
        let e = Package::open(zip(&[("readme.txt", b"hi")])).unwrap_err();
        assert!(e.to_string().contains(MANIFEST), "{e}");
    }

    #[test]
    fn a_bad_uuid_is_rejected() {
        let m = manifest("").replace(UUID, "not-a-uuid");
        let e = Package::open(zip(&[(MANIFEST, m.as_bytes())])).unwrap_err();
        assert!(e.to_string().contains("UUID"), "{e}");
    }

    #[test]
    fn a_newer_min_sdk_is_rejected_with_the_numbers() {
        let m = manifest("").replace(r#""min_sdk_version": 1"#, r#""min_sdk_version": 9"#);
        let e = Package::open(zip(&[(MANIFEST, m.as_bytes())])).unwrap_err();
        let s = e.to_string();
        assert!(s.contains('9') && s.contains('1'), "{s}");
    }

    #[test]
    fn an_override_pointing_at_a_missing_file_is_rejected() {
        let extra = r#""file_overrides": [{"path": "textures/terrain.png"}]"#;
        let e = Package::open(pack(extra, &[])).unwrap_err();
        let s = e.to_string();
        assert!(s.contains("textures/terrain.png"), "{s}");
        assert!(s.contains("android/pe/assets/"), "{s}");
    }

    #[test]
    fn an_override_with_a_traversal_path_is_rejected() {
        let extra = r#""file_overrides": [{"path": "../../etc/passwd"}]"#;
        let e = Package::open(pack(extra, &[])).unwrap_err();
        assert!(e.to_string().contains(".."), "{e}");
    }

    #[test]
    fn an_invalid_mode_is_rejected() {
        let extra = r#""text_overrides": [{"path": "lang/en_US.lang", "mode": "sideways"}]"#;
        let files: &[(&str, &[u8])] = &[("android/pe/assets/lang/en_US.lang", b"a=b\n")];
        let e = Package::open(pack(extra, files)).unwrap_err();
        assert!(e.to_string().contains("sideways"), "{e}");
    }

    #[test]
    fn an_invalid_version_pattern_is_caught_at_import() {
        // An unclosed character class: a real typo a packer makes, and one that
        // would otherwise only surface when the game is launched.
        let m = manifest("").replace("\"0\\\\.14\\\\..*\"", "\"0.14[\"");
        let e = Package::open(zip(&[(MANIFEST, m.as_bytes())])).unwrap_err();
        assert!(e.to_string().contains("pattern"), "{e}");
    }

    #[test]
    fn a_non_android_platform_is_skipped_not_rejected() {
        let m = format!(
            r#"{{
                "uuid": "{UUID}", "name": "P", "version_name": "1", "version_code": 1,
                "manifest_version": 1, "min_sdk_version": 1, "target_sdk_version": 1,
                "game_supports": [
                    {{"platform": "ios", "name": "x", "target_game_versions": [".*"]}},
                    {{"platform": "android", "name": "pe", "target_game_versions": ["1\\.0"]}}
                ]
            }}"#
        );
        let p = Package::open(zip(&[(MANIFEST, m.as_bytes())])).unwrap();
        assert_eq!(p.support_names(), vec!["pe"]);
    }

    #[test]
    fn support_selection_uses_full_match_not_substring() {
        let p = Package::open(pack("", &[])).unwrap();
        assert!(p.support_for("0.14.3").unwrap().is_some());
        // "10.14.3" contains "0.14." but is not a full match for `0\.14\..*`.
        assert!(p.support_for("10.14.3").unwrap().is_none());
        assert!(p.support_for("1.0.0").unwrap().is_none());
    }

    #[test]
    fn native_libs_are_found_with_and_without_the_prefix() {
        let extra = r#""native_libs": [{"name": "mymod"}, {"name": "libother.so"}]"#;
        let files: &[(&str, &[u8])] = &[
            ("android/pe/arm64-v8a/libmymod.so", b"\x7fELF"),
            ("android/pe/arm64-v8a/libother.so", b"\x7fELF"),
        ];
        let p = Package::open(pack(extra, files)).unwrap();
        let s = p.support_for("0.14.2").unwrap().unwrap();
        assert_eq!(
            p.native_libs(s, "arm64-v8a"),
            vec![
                "android/pe/arm64-v8a/libmymod.so".to_string(),
                "android/pe/arm64-v8a/libother.so".to_string()
            ]
        );
        assert!(p.native_libs(s, "x86").is_empty());
        assert_eq!(p.shipped_abis(s), vec!["arm64-v8a"]);
    }

    // ------------------------------------------------------------ overrides

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("blre-nmod-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_override_replaces_the_asset() {
        let extra = r#""file_overrides": [{"path": "textures/terrain.png"}]"#;
        let files: &[(&str, &[u8])] = &[("android/pe/assets/textures/terrain.png", b"NEW")];
        let p = Package::open(pack(extra, files)).unwrap();
        let s = p.support_for("0.14.3").unwrap().unwrap();

        let root = tmp("file");
        let dest = root.join("textures/terrain.png");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"OLD").unwrap();

        let applied = apply(&p, s, &root).unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].kind, "file");
        assert_eq!(std::fs::read(&dest).unwrap(), b"NEW");
    }

    #[test]
    fn text_append_and_prepend_land_on_the_right_side() {
        let extra = r#""text_overrides": [
            {"path": "lang/a.lang", "mode": "append"},
            {"path": "lang/b.lang", "mode": "prepend"}
        ]"#;
        let files: &[(&str, &[u8])] = &[
            ("android/pe/assets/lang/a.lang", b"TAIL\n"),
            ("android/pe/assets/lang/b.lang", b"HEAD\n"),
        ];
        let p = Package::open(pack(extra, files)).unwrap();
        let s = p.support_for("0.14.3").unwrap().unwrap();

        let root = tmp("text");
        std::fs::create_dir_all(root.join("lang")).unwrap();
        std::fs::write(root.join("lang/a.lang"), b"BASE\n").unwrap();
        std::fs::write(root.join("lang/b.lang"), b"BASE\n").unwrap();

        apply(&p, s, &root).unwrap();
        assert_eq!(std::fs::read(root.join("lang/a.lang")).unwrap(), b"BASE\nTAIL\n");
        assert_eq!(std::fs::read(root.join("lang/b.lang")).unwrap(), b"HEAD\nBASE\n");
    }

    #[test]
    fn json_merge_keeps_the_game_value_and_adds_the_mod_keys() {
        let mod_json = br#"{"game_wins":"mod","mod_only":1,"list":[1]}"#;
        let game_json = br#"{"game_wins":"game","list":[2]}"#;
        let merged = merge_json(mod_json, game_json).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(v["game_wins"], "game");
        assert_eq!(v["mod_only"], 1);
        assert_eq!(v["list"], serde_json::json!([1, 2]));
    }

    #[test]
    fn json_merge_refuses_mismatched_types() {
        let e = merge_json(br#"{"a":[]}"#, br#"{"a":{}}"#).unwrap_err();
        assert!(e.contains("'a'"), "{e}");
    }

    #[test]
    fn json_merge_of_two_objects_is_ordered_by_the_game_side() {
        let merged = merge_json(br#"{"s":{"x":1,"y":2}}"#, br#"{"s":{"y":9}}"#).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(v["s"]["x"], 1);
        assert_eq!(v["s"]["y"], 9);
    }

    #[test]
    fn json_merge_needs_an_existing_file_for_merge_mode() {
        let extra = r#""json_overrides": [{"path": "data/table.json", "mode": "merge"}]"#;
        let files: &[(&str, &[u8])] = &[("android/pe/assets/data/table.json", br#"{"a":1}"#)];
        let p = Package::open(pack(extra, files)).unwrap();
        let s = p.support_for("0.14.3").unwrap().unwrap();
        let root = tmp("json-missing");
        let e = apply(&p, s, &root).unwrap_err();
        assert!(e.to_string().contains("does not have"), "{e}");
    }

    #[test]
    fn applications_are_reported_in_reference_order() {
        let extra = r#"
            "file_overrides": [{"path": "a.txt"}],
            "json_overrides": [{"path": "b.json"}],
            "text_overrides": [{"path": "c.txt"}]
        "#;
        let files: &[(&str, &[u8])] = &[
            ("android/pe/assets/a.txt", b"A"),
            ("android/pe/assets/b.json", br#"{"x":1}"#),
            ("android/pe/assets/c.txt", b"C"),
        ];
        let p = Package::open(pack(extra, files)).unwrap();
        let s = p.support_for("0.14.3").unwrap().unwrap();
        let root = tmp("order");
        std::fs::write(root.join("b.json"), br#"{"y":2}"#).unwrap();
        let applied = apply(&p, s, &root).unwrap();
        let kinds: Vec<_> = applied.iter().map(|a| a.kind).collect();
        assert_eq!(kinds, vec!["file", "json", "text"]);
    }

    #[test]
    fn icon_bytes_come_out_of_the_package() {
        let m = manifest("").replace(
            r#""author_email": "a@b.c","#,
            r#""author_email": "a@b.c", "icon": "icon.png", "banner": "banner.png","#,
        );
        let bytes = zip(&[
            (MANIFEST, m.as_bytes()),
            ("icon.png", b"\x89PNG-ICON"),
            ("banner.png", b"\x89PNG-BANNER"),
        ]);
        let p = Package::open(bytes).unwrap();
        assert_eq!(p.icon().unwrap(), b"\x89PNG-ICON");
        assert_eq!(p.banner().unwrap(), b"\x89PNG-BANNER");
    }

    #[test]
    fn a_declared_icon_that_is_not_shipped_is_rejected() {
        let m = manifest("").replace(
            r#""author_email": "a@b.c","#,
            r#""author_email": "a@b.c", "icon": "icon.png","#,
        );
        let e = Package::open(zip(&[(MANIFEST, m.as_bytes())])).unwrap_err();
        assert!(e.to_string().contains("icon"), "{e}");
    }
}
