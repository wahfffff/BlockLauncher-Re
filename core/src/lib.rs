//! BlockLauncher-Re core.
//!
//! This crate holds every decision the launcher makes that does not need an
//! Android device: reading an APK, deciding which builds are in the library,
//! reading an nmod package, and computing what to start. The Android UI is a
//! thin shell over these functions, which is what lets the interesting parts be
//! tested on a host machine instead of only on a phone.
//!
//! Deliberately **not** here:
//!
//! * Any code that downloads a game build. Builds are imported from a file the
//!   user already has; see the README.
//! * Any code that touches `dlopen`. Injection lives in the Android module,
//!   because it needs the game's process and its linker namespace.

pub mod apk;
pub mod axml;
pub mod bridge;
pub mod catalog;
pub mod error;
pub mod jni_glue;
pub mod launch;
pub mod legacy;
pub mod mods;
pub mod nmod;
pub mod pack;
pub mod script;
pub mod script_engine;
pub mod version;

#[cfg(test)]
mod tests_support;

pub use catalog::{Catalog, ImportOutcome, Library, VersionEntry};
pub use error::{Error, Result};
pub use launch::{LaunchPlan, ModSlot, RejectedMod, Rejection};
pub use legacy::LegacyMod;
pub use mods::{Entry as ModEntry, Kind as ModKind};
pub use nmod::Package as NmodPackage;
pub use version::{GameRange, GameVersion};

/// Version of this crate, surfaced in the UI's about screen so a bug report can
/// name the core it ran.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
