//! The half of the runtime that has to exist before the game does.
//!
//! Android offers no supported way into another app's process, so the only
//! entry point is the dynamic linker itself: `wrap.<package>` set to
//! `LD_PRELOAD=<library>` makes the linker map this library first, in the
//! target's own process, under the target's own uid and SELinux domain. That is
//! the whole mechanism; everything below is what to do with it.
//!
//! Two facts about *when* this runs decide the shape of this module:
//!
//! * `.init_array` runs before the game exists. At that moment
//!   `libminecraftpe.so` is not mapped, so there is nothing to resolve yet.
//! * the game library is brought in later by `System.loadLibrary`, which on
//!   Android is `android_dlopen_ext`, not `dlopen`. Interposing only `dlopen`
//!   would therefore never fire.
//!
//! So this module installs interposers for both and does its work at the moment
//! the game library is actually loaded — inside the game's own `dlopen` call,
//! where the handle is in hand and every export is available.
//!
//! The interposers forward unconditionally and cheaply; the expensive work only
//! happens when `BLRE_PRELOAD=1` is in the environment, so loading this library
//! as the app's own JNI code costs one `getenv`.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};

const ANDROID_LOG_INFO: c_int = 4;
const ANDROID_LOG_WARN: c_int = 5;

const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type DlopenFn = unsafe extern "C" fn(*const c_char, c_int) -> *mut c_void;
type DlopenExtFn = unsafe extern "C" fn(*const c_char, c_int, *const c_void) -> *mut c_void;

/// The library the hooks live in. Matched as a substring because the loader may
/// be given an absolute path, a bare soname, or a path inside the APK's lib dir.
const TARGET: &str = "libminecraftpe.so";

/// Set once the target has been handled, so a second load (a second game
/// instance in the same process is impossible, but a retry is not) cannot
/// install the same hooks twice.
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

fn enabled() -> bool {
    std::env::var_os("BLRE_PRELOAD").is_some_and(|v| v == "1")
}

// ------------------------------------------------------------------ the work

/// What happens once the game library is mapped.
///
/// The handle comes straight from the `dlopen` that just returned, which is the
/// most reliable handle available: it is exactly the mapping the game will use.
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
                log(&format!(
                    "  {} -> {symbol} @ {addr:p}",
                    point.id
                ));
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

/// Run the post-load work, but never unwind into the loader.
fn inspect(handle: *mut c_void, name: &str) {
    if handle.is_null() || DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    // A panic here would unwind through `dlopen`'s C frame, which is undefined
    // behaviour rather than a crash someone can debug.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        on_game_loaded(handle, name);
    }));
    if result.is_err() {
        log_at(ANDROID_LOG_WARN, "hook resolution panicked; the game is untouched");
    }
}

fn interested(name: *const c_char) -> Option<String> {
    if name.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(name) }.to_str().ok()?;
    if s.contains(TARGET) {
        Some(s.to_owned())
    } else {
        None
    }
}

// ------------------------------------------------------------- interposers

/// ART loads the game's native library through `android_dlopen_ext`; this is the
/// one that actually fires.
#[no_mangle]
pub unsafe extern "C" fn android_dlopen_ext(
    filename: *const c_char,
    flags: c_int,
    extinfo: *const c_void,
) -> *mut c_void {
    let real: DlopenExtFn = unsafe {
        std::mem::transmute(dlsym(RTLD_NEXT, c"android_dlopen_ext".as_ptr()))
    };
    let h = unsafe { real(filename, flags, extinfo) };

    if enabled() {
        if let Some(name) = interested(filename) {
            inspect(h, &name);
        }
    }
    h
}

/// Kept for callers that use plain `dlopen`; harmless when unused.
#[no_mangle]
pub unsafe extern "C" fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void {
    let real: DlopenFn = unsafe { std::mem::transmute(dlsym(RTLD_NEXT, c"dlopen".as_ptr())) };
    let h = unsafe { real(name, flags) };

    if enabled() {
        if let Some(n) = interested(name) {
            inspect(h, &n);
        }
    }
    h
}

/// `.init_array` runs when this library is mapped, which is before the game
/// library exists. It only reports that the entry point was reached.
extern "C" fn on_load() {
    if !enabled() {
        return;
    }
    log(&format!(
        "preloaded into pid {}; waiting for {TARGET}",
        std::process::id()
    ));
}

#[used]
#[link_section = ".init_array"]
static INIT: extern "C" fn() = on_load;
