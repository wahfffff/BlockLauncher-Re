//! Turning "this build, these mods, this device" into the exact thing to start.
//!
//! Planning is separated from launching on purpose: it is pure, it is testable
//! without a device, and it is where every "why did the game start with the
//! wrong mods" question is actually answered. The Android side only executes the
//! plan it is handed.

use crate::catalog::VersionEntry;
use crate::error::{Error, Result};
use crate::nmod::Nmod;
use crate::version::GameVersion;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModSlot {
    pub id: String,
    /// Absolute path of the library, already resolved for the chosen ABI.
    pub lib: String,
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
    Abi { wanted: String, offered: Vec<String> },
    Game { range_max: Option<String>, range_min: Option<String> },
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

pub fn build(
    entry: &VersionEntry,
    game: &GameVersion,
    device_abis: &[String],
    mods: &[Nmod],
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
        if !m.manifest.supports_abi(&abi) {
            rejected.push(RejectedMod {
                id: m.manifest.id.clone(),
                reason: Rejection::Abi {
                    wanted: abi.clone(),
                    offered: m.manifest.abis.clone(),
                },
            });
            continue;
        }
        if !m.manifest.supports_game(game) {
            rejected.push(RejectedMod {
                id: m.manifest.id.clone(),
                reason: Rejection::Game {
                    range_min: m.manifest.min_game.clone(),
                    range_max: m.manifest.max_game.clone(),
                },
            });
            continue;
        }
        match m.lib_path(&abi) {
            Some(p) => enabled.push(ModSlot {
                id: m.manifest.id.clone(),
                lib: p.to_string_lossy().into_owned(),
            }),
            None => rejected.push(RejectedMod {
                id: m.manifest.id.clone(),
                reason: Rejection::Abi {
                    wanted: abi.clone(),
                    offered: m.manifest.abis.clone(),
                },
            }),
        }
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
