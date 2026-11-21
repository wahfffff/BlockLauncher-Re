//! Turning "this build, these mods, this device" into the exact thing to start.
//!
//! Planning is separated from launching on purpose: it is pure, it is testable
//! without a device, and it is where every "why did the game start with the
//! wrong mods" question is actually answered. The Android side only executes the
//! plan it is handed.
//!
//! Mod selection follows the nmod rules rather than a version range: a package
//! declares *regular expressions* over the game's version name, and the first
//! support whose patterns match is the one used. Offsets into the package are
//! reported, not resolved to paths — extracting is the caller's job, because it
//! is the only part that needs a writable directory.

use crate::catalog::VersionEntry;
use crate::error::{Error, Result};
use crate::nmod;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModSlot {
    /// Manifest uuid: stable across package updates, which is why it sorts.
    pub id: String,
    pub name: String,
    /// Zip entries to load, in manifest order, already resolved for the chosen
    /// ABI. Empty for an assets-only mod.
    pub libs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub version_id: String,
    pub package: String,
    pub activity: String,
    pub abi: String,
    pub data_dir: String,
    /// Loaded in this order. Sorted by id so that two identical selections
    /// always produce the same plan — an unordered plan is untestable.
    pub mods: Vec<ModSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The package declares native libraries but none for the chosen ABI.
    Abi { wanted: String, offered: Vec<String> },
    /// No `game_supports` entry matched this build's version name.
    Version { wanted: String, supports: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedMod {
    pub id: String,
    pub reason: Rejection,
}

/// Activity to start when the manifest did not advertise a MAIN activity.
/// Legacy Minecraft PE builds are all `com.mojang.minecraftpe.MainActivity`, and
/// a launcher that refuses to guess here would fail on every one of them.
const FALLBACK_ACTIVITY: &str = "MainActivity";

fn pick_abi(entry: &VersionEntry, device_abis: &[String]) -> Result<String> {
    // Prefer the device's own ordering: it already encodes whether this is a
    // 64-bit device, which a hardcoded preference list would get wrong on some
    // vendor ROMs.
    for abi in device_abis {
        if entry.abis.iter().any(|a| a == abi) {
            return Ok(abi.clone());
        }
    }
    Err(Error::UnusableManifest(format!(
        "build {} offers [{}] but the device has [{}]",
        entry.id,
        if entry.abis.is_empty() {
            "no native code".to_string()
        } else {
            entry.abis.join(", ")
        },
        device_abis.join(", ")
    )))
}

/// Plan a launch.
///
/// `mods` are already opened packages; a package that fails to open never gets
/// this far (`nmod::Package::open` is where that is reported). Everything here
/// is a decision about *this* build and *this* device.
pub fn build(
    entry: &VersionEntry,
    device_abis: &[String],
    mods: &[&nmod::Package],
    data_dir: &str,
) -> Result<(LaunchPlan, Vec<RejectedMod>)> {
    if device_abis.is_empty() {
        return Err(Error::UnusableManifest(
            "the device reported no supported ABI".into(),
        ));
    }
    let abi = pick_abi(entry, device_abis)?;

    let activity = match &entry.launcher_activity {
        Some(a) if !a.is_empty() => {
            // Manifests sometimes store the bare class name relative to the
            // package; Android needs the qualified form.
            if a.contains('.') {
                a.clone()
            } else {
                format!("{}.{}", entry.package, a)
            }
        }
        _ => format!("{}.{}", entry.package, FALLBACK_ACTIVITY),
    };

    let mut enabled: Vec<ModSlot> = Vec::new();
    let mut rejected: Vec<RejectedMod> = Vec::new();

    for m in mods {
        let support = match m.support_for(&entry.version_name)? {
            Some(s) => s,
            None => {
                rejected.push(RejectedMod {
                    id: m.uuid().to_string(),
                    reason: Rejection::Version {
                        wanted: entry.version_name.clone(),
                        supports: m.support_names().iter().map(|s| s.to_string()).collect(),
                    },
                });
                continue;
            }
        };

        let libs = m.native_libs(support, &abi);
        let declared = support.native_libs.as_deref().unwrap_or(&[]);
        if !declared.is_empty() && libs.is_empty() {
            rejected.push(RejectedMod {
                id: m.uuid().to_string(),
                reason: Rejection::Abi {
                    wanted: abi.clone(),
                    offered: m.shipped_abis(support),
                },
            });
            continue;
        }

        enabled.push(ModSlot {
            id: m.uuid().to_string(),
            name: m.name().to_string(),
            libs,
        });
    }

    enabled.sort_by(|a, b| a.id.cmp(&b.id));
    rejected.sort_by(|a, b| a.id.cmp(&b.id));

    let plan = LaunchPlan {
        version_id: entry.id.clone(),
        package: entry.package.clone(),
        activity,
        abi,
        data_dir: data_dir.to_string(),
        mods: enabled,
    };
    Ok((plan, rejected))
}

/// Where a package should be extracted to before its overrides are applied.
///
/// Kept here rather than in `nmod` because it is a layout decision of the
/// launcher, not a property of the format.
pub fn assets_dir(instance_root: &std::path::Path) -> PathBuf {
    instance_root.join("nmods-assets")
}
