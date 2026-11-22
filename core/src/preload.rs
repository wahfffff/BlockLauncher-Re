//! The half of the runtime that has to exist before the game does.
//!
//! Android offers no supported way into another app's process, so the only
//! entry point is the dynamic linker itself: `wrap.<package>` set to
//! `LD_PRELOAD=<library>` makes the linker map this library first, in the
//! target's own process, under the target's own uid and SELinux domain.
//!
//! `.init_array` runs before the game exists, so the game library is not mapped
//! yet; it arrives later through `System.loadLibrary`. The whole job here is to
//! be in the right function at that moment.
//!
//! # Why it forwards to `__loader_*`
//!
//! bionic keeps the real implementations in symbols named differently from the
//! ones it exports for callers: `dlopen` is `__loader_dlopen`,
//! `android_dlopen_ext` is `__loader_android_dlopen_ext`. Forwarding to those
//! directly is the only approach here that cannot recurse, and two earlier
//! attempts proved why the obvious ones do not work:
//!
//! * `dlsym(RTLD_NEXT, "android_dlopen_ext")` resolved back to this very
//!   function, and the forward became unbounded recursion:
//!   `Fatal signal 11 ... Cause: stack pointer is close to top of stack`;
//! * resolving through a `libdl` handle needed `dlopen` to obtain the handle,
//!   which is the function being interposed.
//!
//! Each `__loader_*` entry takes one extra argument, the address of the caller,
//! which bionic uses for namespace and `RTLD_NEXT` bookkeeping. Passing this
//! library's own function address is the truthful answer.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const ANDROID_LOG_INFO: c_int = 4;
const ANDROID_LOG_WARN: c_int = 5;

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;

    /// The linker's own `dlopen`, not the exported alias.
    fn __loader_dlopen(name: *const c_char, flags: c_int, caller: *const c_void) -> *mut c_void;

    /// The linker's own `android_dlopen_ext`.
    fn __loader_android_dlopen_ext(
        name: *const c_char,
        flags: c_int,
        info: *const c_void,
        caller: *const c_void,
    ) -> *mut c_void;
}

/// The loader may be handed a bare soname, an absolute path in the APK's lib
/// directory, or a path inside another namespace. All of them end in this.
const TARGET: &str = "libminecraftpe.so";

/// Re-entry guard. If the loader ends up calling back into us while we are
/// inside it, the nested call must pass straight through.
static IN_CALL: AtomicBool = AtomicBool::new(false);

/// Set once the target has been handled, so a second load cannot install the
/// same hooks twice.
static DONE: AtomicBool = AtomicBool::new(false);

/// How many load requests have been logged. The diagnostic is bounded so a game
/// that loads a hundred libraries does not flood logcat.
static LOGGED: AtomicUsize = AtomicUsize::new(0);
const LOG_LIMIT: usize = 60;

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

// ------------------------------------------------------------------ the work

/// What happens once the game library is mapped.
///
/// The handle comes from the load that just returned, which is the most reliable
/// handle available: it is exactly the mapping the game will use.
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

/// Run the post-load work, but never unwind into the loader: a panic here would
/// cross a C frame, which is undefined behaviour rather than a diagnosable crash.
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

fn name_of(name: *const c_char) -> Option<String> {
    if name.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(name) }.to_str().ok().map(str::to_owned)
}

/// Every load the game performs is worth one line while this is being brought
/// up: it is how we learned which entry point the game actually uses.
fn note_load(kind: &str, name: *const c_char, ok: bool) {
    if !enabled() {
        return;
    }
    let n = LOGGED.fetch_add(1, Ordering::Relaxed);
    if n >= LOG_LIMIT {
        return;
    }
    let what = name_of(name).unwrap_or_else(|| "<null>".to_string());
    log(&format!(
        "{kind}({what}) -> {}",
        if ok { "ok" } else { "failed" }
    ));
}

// ----------------------------------------------------------- the interposers

/// ART loads the game's native library through `android_dlopen_ext`.
#[no_mangle]
pub unsafe extern "C" fn android_dlopen_ext(
    filename: *const c_char,
    flags: c_int,
    extinfo: *const c_void,
) -> *mut c_void {
    let caller = android_dlopen_ext as *const () as *const c_void;

    let reentered = IN_CALL.swap(true, Ordering::SeqCst);
    let h = unsafe { __loader_android_dlopen_ext(filename, flags, extinfo, caller) };
    IN_CALL.store(reentered, Ordering::SeqCst);

    note_load("android_dlopen_ext", filename, !h.is_null());
    if !reentered && !h.is_null() {
        if let Some(n) = name_of(filename) {
            if n.contains(TARGET) {
                inspect(h, &n);
            }
        }
    }
    h
}

/// Some libraries are opened with plain `dlopen`, and the forward target here is
/// the loader's own symbol, so this cannot recurse — which is what made the
/// first attempt at interposing `dlopen` crash the process.
#[no_mangle]
pub unsafe extern "C" fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void {
    let caller = dlopen as *const () as *const c_void;

    let reentered = IN_CALL.swap(true, Ordering::SeqCst);
    let h = unsafe { __loader_dlopen(name, flags, caller) };
    IN_CALL.store(reentered, Ordering::SeqCst);

    note_load("dlopen", name, !h.is_null());
    if !reentered && !h.is_null() {
        if let Some(n) = name_of(name) {
            if n.contains(TARGET) {
                inspect(h, &n);
            }
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
