//! The half of the runtime that has to exist before the game does.
//!
//! Android offers no supported way into another app's process, so the only entry
//! point is the dynamic linker: `wrap.<package>` set to `LD_PRELOAD=<library>`
//! makes the linker map this library first, in the target's own process, under
//! the target's own uid and SELinux domain.
//!
//! `.init_array` runs before the game exists, so the game library is not mapped
//! yet and there is nothing to resolve. Something has to run later, when it is.
//!
//! # Why this watches `/proc/self/maps` instead of interposing `dlopen`
//!
//! Getting a callback at that moment means exporting `dlopen` or
//! `android_dlopen_ext` and forwarding to the linker's own version. Three
//! attempts, three distinct failures, all recorded here because they are the
//! reason this file looks the way it does:
//!
//! 1. `dlsym(RTLD_NEXT, ...)` resolved back to this function, so the forward
//!    became unbounded recursion: `Fatal signal 11 ... Cause: stack pointer is
//!    close to top of stack`.
//! 2. Resolving through a `libdl` handle needs `dlopen` to obtain the handle —
//!    the very function being interposed.
//! 3. Forwarding to `__loader_dlopen`/`__loader_android_dlopen_ext`, which take
//!    the caller's address so the linker can pick the right namespace. Passing
//!    this library's address made `libart.so` unloadable from the runtime's
//!    namespace and the process aborted (`SIGABRT`); recovering the true caller
//!    needs a naked function and hand-written ARM assembly to read `lr` before
//!    the prologue clobbers it.
//!
//! Watching the mapping table exports no symbols at all, so none of those
//! failure modes exist. The trade is stated plainly: hooks are installed when
//! the library appears — milliseconds after it is loaded, **not before the first
//! instruction of the game runs**. A hook target called inside the game's own
//! `JNI_OnLoad` would be missed. Nothing in the ModPE surface is reachable that
//! early, but that is a real cost, not zero.

use std::ffi::{c_int, c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const ANDROID_LOG_INFO: c_int = 4;
const ANDROID_LOG_WARN: c_int = 5;

const RTLD_NOW: c_int = 2;
const RTLD_NOLOAD: c_int = 4;

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

use std::ffi::c_char;

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
}

/// The library the hooks live in.
const TARGET: &str = "libminecraftpe.so";

/// How long to keep watching for it.
const WATCH_SECS: u32 = 30;
const POLL_MS: u32 = 10;

/// Set once the target has been handled, so a second sighting cannot install the
/// same hooks twice.
static DONE: AtomicBool = AtomicBool::new(false);

fn log_at(prio: c_int, msg: &str) {
    if let Ok(text) = CString::new(msg) {
        unsafe {
            __android_log_write(prio, c"blre".as_ptr(), text.as_ptr());
        }
    }
}

fn log(msg: &str) {
    log_at(ANDROID_LOG_INFO, msg);
}

/// The work is opt-in: loading this library as the app's own JNI code must cost
/// nothing more than one `getenv`.
fn enabled() -> bool {
    std::env::var_os("BLRE_PRELOAD").is_some_and(|v| v == "1")
}

// ---------------------------------------------------------------- the watch

/// The path of the game library as this process has it mapped, if any.
///
/// The path from `/proc/self/maps` is preferred over the bare soname because it
/// is the exact file the linker used, and `dlopen` will accept it without having
/// to search a namespace for a name.
fn mapped_path() -> Option<String> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    for line in maps.lines() {
        let Some(path) = line.split_whitespace().last() else {
            continue;
        };
        if path.contains(TARGET) && path.starts_with('/') {
            return Some(path.to_string());
        }
    }
    None
}

// ------------------------------------------------------------------ the work

/// What happens once the game library is mapped.
fn on_game_loaded(handle: *mut c_void, name: &str) {
    log(&format!("game library loaded: {name}"));

    let mut found = 0usize;
    let mut missing: Vec<&str> = Vec::new();

    for point in crate::hook::POINTS {
        let mut hit = None;
        for candidate in point.candidates {
            let Ok(sym) = CString::new(*candidate) else {
                continue;
            };
            let addr = unsafe { dlsym(handle, sym.as_ptr()) };
            if !addr.is_null() {
                hit = Some((*candidate, addr));
                break;
            }
        }
        match hit {
            Some((symbol, addr)) => {
                found += 1;
                log(&format!("  {} -> {symbol} @ {addr:p}", point.id));
            }
            None => {
                missing.push(point.id);
                log_at(
                    ANDROID_LOG_WARN,
                    &format!("  {} -> MISSING (tried {:?})", point.id, point.candidates),
                );
            }
        }
    }

    log(&format!(
        "resolved {found}/{} hook points{}",
        crate::hook::POINTS.len(),
        if missing.is_empty() {
            String::new()
        } else {
            format!("; missing: {}", missing.join(", "))
        }
    ));
}

/// Run the post-load work, but never unwind across a foreign frame: a panic here
/// would be undefined behaviour rather than a diagnosable crash.
fn inspect(handle: *mut c_void, name: &str) {
    if handle.is_null() || DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        on_game_loaded(handle, name);
    }));
    if result.is_err() {
        log_at(
            ANDROID_LOG_WARN,
            "hook resolution panicked; the game itself is untouched",
        );
    }
}

/// The watch loop, on its own thread so it cannot delay process start.
fn watch() {
    let deadline = std::time::Instant::now() + Duration::from_secs(WATCH_SECS as u64);
    while std::time::Instant::now() < deadline {
        if let Some(path) = mapped_path() {
            let Ok(c) = CString::new(path.clone()) else {
                return;
            };
            // RTLD_NOLOAD: it is already mapped, so this must not load anything;
            // it only turns the mapping into a handle `dlsym` accepts.
            let handle = unsafe { dlopen(c.as_ptr(), RTLD_NOW | RTLD_NOLOAD) };
            if handle.is_null() {
                log_at(
                    ANDROID_LOG_WARN,
                    &format!("{path} is mapped but dlopen(RTLD_NOLOAD) returned null"),
                );
            } else {
                inspect(handle, &path);
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(POLL_MS as u64));
    }
    log_at(
        ANDROID_LOG_WARN,
        &format!("{TARGET} did not appear within {WATCH_SECS}s"),
    );
}

/// `.init_array` runs when this library is mapped, which is before the game
/// library exists. It starts the watch and returns immediately.
extern "C" fn on_load() {
    if !enabled() {
        return;
    }
    log(&format!(
        "preloaded into pid {}; watching for {TARGET}",
        std::process::id()
    ));
    // Nothing joins this thread; the process owns it from here.
    let _ = std::thread::Builder::new()
        .name("blre-watch".into())
        .spawn(watch);
}

#[used]
#[link_section = ".init_array"]
static INIT: extern "C" fn() = on_load;
