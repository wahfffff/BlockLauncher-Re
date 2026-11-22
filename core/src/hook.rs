//! Which function in the game a ModPE call has to reach, and whether this build
//! actually exports it.
//!
//! This is the part of injection that can be decided without a phone. Resolving
//! a hook point is a lookup in the library's dynamic symbol table; doing it here
//! means the launcher can refuse *before* starting the game, with a message that
//! names the symbol, instead of crashing inside `dlopen` or, worse, silently
//! calling into the wrong function because a name happened to match.
//!
//! ## Why the names are spelled out rather than derived
//!
//! A ModPE call like `Level.setTile` does not correspond to a function with that
//! name in the game. In the 0.13/0.14.3 library this was measured against, there
//! is no `Level::setTile` at all — the only `setTile` is
//! `SpriteComponent::setTiled(bool)`, which is unrelated. The real path is
//! `BlockSource::setBlockAndData(int, int, int, BlockID, unsigned char, int)`.
//!
//! So each point lists the mangled names it will accept, and a new game family
//! is handled by adding a candidate — not by loosening the match. Guessing from
//! a substring would resolve `getTile` to `SpriteComponent::getTiled` and hook a
//! sprite renderer.
//!
//! ## What "required" means
//!
//! A plan with a missing required point is not usable: the launcher says which
//! ModPE API will not work and declines to claim injection. Optional points
//! degrade one feature each and are reported, not fatal.

use serde::Serialize;

use crate::elf::{self, Symbol};
use crate::error::Result;

/// One thing the injected runtime has to reach inside the game.
#[derive(Debug)]
pub struct Point {
    /// Stable id, used in JSON and in error messages.
    pub id: &'static str,
    /// The ModPE API this point is the implementation of, for the user-facing
    /// sentence. Several APIs can share a point.
    pub modpe: &'static str,
    /// Mangled names accepted for this point, best first.
    pub candidates: &'static [&'static str],
    /// Whether the launcher refuses to claim injection when this is absent.
    pub required: bool,
}

/// The points this build knows how to resolve.
///
/// Every name below was read out of `lib/armeabi-v7a/libminecraftpe.so` of a real
/// Pocket Edition 0.13/0.14.3 build (30,992 dynamic symbols), not invented. The
/// `arm64` side of the same release family is not covered yet because no 64-bit
/// build of this era was available to check.
pub const POINTS: &[Point] = &[
    Point {
        id: "blocksource.set",
        modpe: "Level.setTile / Level.setBlock",
        candidates: &["_ZN11BlockSource15setBlockAndDataEiii7BlockIDhi"],
        required: true,
    },
    Point {
        id: "blocksource.get",
        modpe: "Level.getTile / Level.getBlock",
        candidates: &["_ZN11BlockSource8getBlockERK8BlockPos"],
        required: true,
    },
    Point {
        id: "level.explode",
        modpe: "Level.explode",
        candidates: &["_ZN5Level7explodeER11BlockSourceP6EntityRK4Vec3fb"],
        required: true,
    },
    Point {
        id: "gamemode.attack",
        modpe: "Player.attack / the attack callback",
        candidates: &["_ZN8GameMode6attackER6PlayerR6Entity", "_ZN6Player6attackER6Entity"],
        required: true,
    },
    Point {
        id: "gamemode.destroy",
        modpe: "blockEventCallback, destroyBlock",
        candidates: &[
            "_ZN8GameMode12destroyBlockER6Player8BlockPosa",
            "_ZN12SurvivalMode12destroyBlockER6Player8BlockPosa",
        ],
        required: true,
    },
    Point {
        id: "level.set_time",
        modpe: "Level.setTime",
        candidates: &["_ZN5Level7setTimeEi"],
        required: false,
    },
    Point {
        id: "level.get_time",
        modpe: "Level.getTime",
        candidates: &["_ZNK5Level7getTimeEv"],
        required: false,
    },
    Point {
        id: "renderer.fov",
        modpe: "the FOV getter used by ModPE camera scripts",
        candidates: &["_ZN13LevelRenderer6getFovEfb"],
        required: false,
    },
    Point {
        id: "block.registry",
        modpe: "Block.getAllBlockIds",
        candidates: &["_ZN5Block7mBlocksE"],
        required: false,
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct Resolved {
    pub id: &'static str,
    pub modpe: &'static str,
    pub symbol: String,
    /// Offset inside the library, which is where a hook would be placed once it
    /// is loaded. Not a runtime address: that is only known on the device.
    pub value: u64,
    pub size: u64,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Missing {
    pub id: &'static str,
    pub modpe: &'static str,
    pub required: bool,
    /// What was tried, so a report can say what to add for a new version rather
    /// than "symbol not found".
    pub tried: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    /// Empty for a plan built from names alone, before the file was parsed.
    pub arch: String,
    pub symbol_count: usize,
    pub resolved: Vec<Resolved>,
    pub missing: Vec<Missing>,
}

impl Plan {
    /// True when every required point resolved. Optional gaps do not stop the
    /// game from starting; they only make one API unavailable.
    pub fn usable(&self) -> bool {
        !self.missing.iter().any(|m| m.required)
    }

    /// One sentence a UI can show instead of a boolean.
    pub fn summary(&self) -> String {
        if self.usable() {
            let optional = self.missing.len();
            if optional == 0 {
                format!("{} hook points resolved", self.resolved.len())
            } else {
                format!(
                    "{} hook points resolved, {optional} optional API(s) unavailable",
                    self.resolved.len()
                )
            }
        } else {
            let names: Vec<&str> = self
                .missing
                .iter()
                .filter(|m| m.required)
                .map(|m| m.modpe)
                .collect();
            format!(
                "this build is missing {} required hook point(s): {}",
                names.len(),
                names.join(", ")
            )
        }
    }
}

/// Decide the plan from a symbol table.
///
/// Separate from [`plan`] so the resolution rules can be tested against
/// hand-written symbol lists, including the case that matters most: a library
/// that exports `SpriteComponent::getTiled` but not `BlockSource::getBlock`.
pub fn resolve(symbols: &[Symbol]) -> Plan {
    let mut resolved = Vec::new();
    let mut missing = Vec::new();

    for point in POINTS {
        let hit = point
            .candidates
            .iter()
            .find_map(|want| symbols.iter().find(|s| s.name == *want && s.is_exported()));

        match hit {
            Some(s) => resolved.push(Resolved {
                id: point.id,
                modpe: point.modpe,
                symbol: s.name.clone(),
                value: s.value,
                size: s.size,
                required: point.required,
            }),
            None => missing.push(Missing {
                id: point.id,
                modpe: point.modpe,
                required: point.required,
                tried: point.candidates.to_vec(),
            }),
        }
    }

    Plan {
        arch: String::new(),
        symbol_count: 0,
        resolved,
        missing,
    }
}

/// Parse a shared object and resolve the plan against it.
pub fn plan(so: &[u8]) -> Result<Plan> {
    let (header, symbols) = elf::parse(so)?;
    let mut p = resolve(&symbols);
    p.arch = header.arch_name().to_string();
    p.symbol_count = header.symbol_count;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::SymbolType;

    fn sym(name: &str, value: u64, size: u64, section: u16) -> Symbol {
        Symbol {
            name: name.to_string(),
            value,
            size,
            kind: SymbolType::FUNC,
            section,
        }
    }

    /// Every POINTS candidate that a real 0.14.3 library exports. Keeping the
    /// list explicit means a typo in a mangled name shows up here rather than on
    /// a phone.
    fn real_014() -> Vec<Symbol> {
        let mut v = vec![
            sym("_ZN11BlockSource15setBlockAndDataEiii7BlockIDhi", 0x1000, 96, 7),
            sym("_ZN11BlockSource8getBlockERK8BlockPos", 0x1100, 64, 7),
            sym("_ZN5Level7explodeER11BlockSourceP6EntityRK4Vec3fb", 0x1200, 512, 7),
            sym("_ZN8GameMode6attackER6PlayerR6Entity", 0x1300, 256, 7),
            sym("_ZN8GameMode12destroyBlockER6Player8BlockPosa", 0x1400, 384, 7),
            sym("_ZN5Level7setTimeEi", 0x1500, 32, 7),
            sym("_ZNK5Level7getTimeEv", 0x1600, 16, 7),
            sym("_ZN13LevelRenderer6getFovEfb", 0x1700, 128, 7),
            sym("_ZN5Block7mBlocksE", 0x2000, 4, 25),
        ];
        // Names that look similar and must not be mistaken for the real ones.
        v.push(sym("_ZNK15SpriteComponent8getTiledEv", 0x1800, 8, 7));
        v.push(sym("_ZN15SpriteComponent8setTiledEb", 0x1900, 8, 7));
        v
    }

    #[test]
    fn a_real_014_library_resolves_every_point() {
        let p = resolve(&real_014());
        assert!(p.usable(), "{:?}", p.missing);
        assert_eq!(p.resolved.len(), POINTS.len());
        assert!(p.missing.is_empty());
    }

    #[test]
    fn a_lookalike_name_is_not_a_hook_point() {
        // The sprite component's getTiled/setTiled are the only names containing
        // "getTile"/"setTile" in the real library. A substring match would pick
        // them up; exact matching must not.
        let only_sprite = vec![
            sym("_ZNK15SpriteComponent8getTiledEv", 0x1800, 8, 7),
            sym("_ZN15SpriteComponent8setTiledEb", 0x1900, 8, 7),
        ];
        let p = resolve(&only_sprite);
        assert!(!p.usable());
        let ids: Vec<&str> = p.missing.iter().map(|m| m.id).collect();
        assert!(ids.contains(&"blocksource.get"));
        assert!(ids.contains(&"blocksource.set"));
        assert!(p.resolved.is_empty());
    }

    #[test]
    fn an_imported_symbol_does_not_count_as_exported() {
        // section 0 is SHN_UNDEF: the library asks its loader for this symbol.
        let undefined = vec![sym("_ZN5Level7explodeER11BlockSourceP6EntityRK4Vec3fb", 0, 0, 0)];
        let p = resolve(&undefined);
        assert!(p.missing.iter().any(|m| m.id == "level.explode"));
    }

    #[test]
    fn a_missing_optional_point_is_reported_but_not_fatal() {
        let mut syms = real_014();
        syms.retain(|s| s.name != "_ZN13LevelRenderer6getFovEfb");
        let p = resolve(&syms);
        assert!(p.usable(), "an optional gap must not refuse injection");
        assert!(p.missing.iter().any(|m| m.id == "renderer.fov" && !m.required));
        assert!(p.summary().contains("optional"), "{}", p.summary());
    }

    #[test]
    fn a_missing_required_point_names_the_modpe_api() {
        let mut syms = real_014();
        syms.retain(|s| s.name != "_ZN5Level7explodeER11BlockSourceP6EntityRK4Vec3fb");
        let p = resolve(&syms);
        assert!(!p.usable());
        let s = p.summary();
        assert!(s.contains("Level.explode"), "{s}");
    }

    #[test]
    fn resolution_is_stable_regardless_of_symbol_order() {
        let mut syms = real_014();
        let a = resolve(&syms);
        syms.reverse();
        let b = resolve(&syms);
        let ids_a: Vec<&str> = a.resolved.iter().map(|r| r.id).collect();
        let ids_b: Vec<&str> = b.resolved.iter().map(|r| r.id).collect();
        assert_eq!(ids_a, ids_b);
    }

    #[test]
    fn a_non_elf_is_refused_with_a_reason() {
        let e = plan(b"definitely not an elf").unwrap_err();
        assert!(e.to_string().contains("ELF"), "{e}");
    }
}
