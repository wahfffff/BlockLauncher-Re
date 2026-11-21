//! The ModPE script runtime.
//!
//! BlockLauncher's scripts are JavaScript calling a fixed set of globals —
//! `Level`, `Player`, `Entity`, `Block`, `ModPE` and a few free functions. The
//! names in [`API`] come from the launcher's own dex, read as signatures only
//! by `tools/modpe_api.py`; the calling conventions are the documented ones.
//!
//! ## The seam
//!
//! Every global is a thin adapter over [`Host`]. The core therefore contains no
//! game access at all — it contains a *contract*, and the Android side supplies
//! an implementation that talks to the running game. That has two consequences
//! worth stating:
//!
//! * Script semantics can be tested on a host machine against a fake world. The
//!   tests below are the specification.
//! * With no game attached ([`NullHost`]) the globals do not silently no-op.
//!   They throw. A script that half-works is worse to debug than one that says
//!   "this launcher cannot answer that yet".
//!
//! ## What is not here
//!
//! There is no instruction budget: a `while (true)` in a script blocks the
//! calling thread. That is not an oversight — it is why scripts run on the
//! launcher's script thread and not the UI thread — but there is no deadline
//! enforcement yet, and a script that spins will be killed with the process
//! rather than caught. Saying so beats pretending.

use std::cell::RefCell;

/// Everything a script can reach in the game. Implemented by the Android
/// module; `NullHost` stands in when there is no game.
pub trait Host {
    fn log(&mut self, level: &str, message: &str);

    fn minecraft_version(&mut self) -> std::result::Result<String, String>;

    fn get_tile(&mut self, x: i32, y: i32, z: i32) -> std::result::Result<i32, String>;
    fn set_tile(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        id: i32,
        meta: i32,
    ) -> std::result::Result<(), String>;

    fn player_position(&mut self) -> std::result::Result<(f64, f64, f64), String>;
    fn set_player_position(&mut self, x: f64, y: f64, z: f64) -> std::result::Result<(), String>;

    fn entity_count(&mut self) -> std::result::Result<usize, String>;

    /// `ModPE.saveData` / `readData` / `removeData`. `Ok(None)` means "no value".
    fn data_get(&mut self, key: &str) -> std::result::Result<Option<String>, String>;
    fn data_set(&mut self, key: &str, value: Option<&str>) -> std::result::Result<(), String>;
}

/// The host used when no game is running. Every game call fails with the same
/// sentence, because the reason is always the same one.
pub struct NullHost {
    lines: Vec<String>,
}

impl Default for NullHost {
    fn default() -> Self {
        NullHost { lines: Vec::new() }
    }
}

impl NullHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}

const NO_GAME: &str = "no game is attached: this launcher cannot answer that yet";

impl Host for NullHost {
    fn log(&mut self, level: &str, message: &str) {
        self.lines.push(format!("[{level}] {message}"));
    }
    fn minecraft_version(&mut self) -> std::result::Result<String, String> {
        Err(NO_GAME.into())
    }
    fn get_tile(&mut self, _: i32, _: i32, _: i32) -> std::result::Result<i32, String> {
        Err(NO_GAME.into())
    }
    fn set_tile(&mut self, _: i32, _: i32, _: i32, _: i32, _: i32) -> std::result::Result<(), String> {
        Err(NO_GAME.into())
    }
    fn player_position(&mut self) -> std::result::Result<(f64, f64, f64), String> {
        Err(NO_GAME.into())
    }
    fn set_player_position(&mut self, _: f64, _: f64, _: f64) -> std::result::Result<(), String> {
        Err(NO_GAME.into())
    }
    fn entity_count(&mut self) -> std::result::Result<usize, String> {
        Err(NO_GAME.into())
    }
    fn data_get(&mut self, _: &str) -> std::result::Result<Option<String>, String> {
        Err(NO_GAME.into())
    }
    fn data_set(&mut self, _: &str, _: Option<&str>) -> std::result::Result<(), String> {
        Err(NO_GAME.into())
    }
}

/// The globals this runtime provides, and the arity it accepts.
///
/// Kept as data rather than as prose so `docs/MODPE-API.md` and the tests can
/// both be checked against one list. Names are `<namespace>.<member>`, or a
/// bare name for a free function.
pub const API: &[(&str, usize)] = &[
    ("print", 1),
    ("clientMessage", 1),
    ("Level.getTile", 3),
    ("Level.setTile", 5),
    ("Level.getMinecraftVersion", 0),
    ("Level.explode", 4),
    ("Player.getX", 0),
    ("Player.getY", 0),
    ("Player.getZ", 0),
    ("Player.setPosition", 3),
    ("Entity.getAll", 0),
    ("ModPE.readData", 1),
    ("ModPE.saveData", 2),
    ("ModPE.removeData", 1),
];

/// Names a script may define and the launcher will call.
pub const HOOKS: &[&str] = &["newLevel", "modTick", "chatHook", "selectLevelHook"];

/// A loaded script and the errors it produced.
#[derive(Debug, Default)]
pub struct Report {
    pub ran: usize,
    pub failed: usize,
    pub messages: Vec<String>,
}

thread_local! {
    /// The host for the duration of a call. A thread-local rather than context
    /// data because the alternative is teaching the GC about a borrowed
    /// `&mut dyn Host`, and a `while` loop over that is not worth it.
    static ACTIVE: RefCell<Vec<Box<dyn Host>>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn with_host<T>(f: impl FnOnce(&mut dyn Host) -> std::result::Result<T, String>) -> std::result::Result<T, String> {
    ACTIVE.with(|slot| {
        let mut slot = slot.borrow_mut();
        match slot.last_mut() {
            Some(host) => f(host.as_mut()),
            None => Err("script runtime used without a host".to_string()),
        }
    })
}

/// Installs `host` as the thread's host until the guard drops.
pub(crate) struct ActiveGuard;

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.with(|slot| {
            slot.borrow_mut().pop();
        });
    }
}

pub(crate) fn enter(host: Box<dyn Host>) -> ActiveGuard {
    ACTIVE.with(|slot| slot.borrow_mut().push(host));
    ActiveGuard
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeHost {
        tiles: std::collections::HashMap<(i32, i32, i32), i32>,
        pos: (f64, f64, f64),
        data: std::collections::HashMap<String, String>,
        log: Vec<String>,
    }

    impl Default for FakeHost {
        fn default() -> Self {
            FakeHost {
                tiles: Default::default(),
                pos: (0.5, 64.0, 0.5),
                data: Default::default(),
                log: Vec::new(),
            }
        }
    }

    impl Host for FakeHost {
        fn log(&mut self, level: &str, message: &str) {
            self.log.push(format!("[{level}] {message}"));
        }
        fn minecraft_version(&mut self) -> std::result::Result<String, String> {
            Ok("0.14.3".into())
        }
        fn get_tile(&mut self, x: i32, y: i32, z: i32) -> std::result::Result<i32, String> {
            Ok(*self.tiles.get(&(x, y, z)).unwrap_or(&0))
        }
        fn set_tile(&mut self, x: i32, y: i32, z: i32, id: i32, meta: i32) -> std::result::Result<(), String> {
            if !(0..=255).contains(&id) {
                return Err(format!("tile id {id} is out of range"));
            }
            let _ = meta;
            self.tiles.insert((x, y, z), id);
            Ok(())
        }
        fn player_position(&mut self) -> std::result::Result<(f64, f64, f64), String> {
            Ok(self.pos)
        }
        fn set_player_position(&mut self, x: f64, y: f64, z: f64) -> std::result::Result<(), String> {
            self.pos = (x, y, z);
            Ok(())
        }
        fn entity_count(&mut self) -> std::result::Result<usize, String> {
            Ok(0)
        }
        fn data_get(&mut self, key: &str) -> std::result::Result<Option<String>, String> {
            Ok(self.data.get(key).cloned())
        }
        fn data_set(&mut self, key: &str, value: Option<&str>) -> std::result::Result<(), String> {
            match value {
                Some(v) => {
                    self.data.insert(key.to_string(), v.to_string());
                }
                None => {
                    self.data.remove(key);
                }
            }
            Ok(())
        }
    }

    #[test]
    fn the_api_table_has_no_duplicate_names() {
        let mut seen = std::collections::HashSet::new();
        for (name, _) in API {
            assert!(seen.insert(*name), "duplicate API entry: {name}");
        }
    }

    #[test]
    fn null_host_reports_rather_than_pretending() {
        let mut h = NullHost::new();
        h.log("info", "hello");
        assert_eq!(h.lines().len(), 1);
        assert!(h.get_tile(0, 0, 0).is_err());
    }

    #[test]
    fn fake_host_records_what_a_direct_call_does() {
        let mut h = FakeHost::default();
        h.set_tile(1, 2, 3, 4, 0).unwrap();
        assert_eq!(h.get_tile(1, 2, 3).unwrap(), 4);
        assert!(h.set_tile(0, 0, 0, 999, 0).is_err());
    }
}
