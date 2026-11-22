//! The half of the runtime that has to exist before the game does.
//!
//! Android offers no supported way into another app's process, so the only
//! entry point is the dynamic linker itself: `wrap.<package>` set to
//! `LD_PRELOAD=<library>` makes the linker map this library first, in the
//! target's own process, under the target's own uid and SELinux domain.
//!
//! Two facts about *when* this runs decide the shape of this module:
//!
//! * `.init_array` runs before the game exists, so `libminecraftpe.so` is not
//!   mapped yet and there is nothing to resolve;
//! * the game library arrives later through `System.loadLibrary`, which on
//!   Android is `android_dlopen_ext` — not `dlopen`.
//!
//! Only `android_dlopen_ext` is interposed, and the real function is taken from
//! `libdl`'s own handle rather than through `RTLD_NEXT`. Both of those are
//! recorded because both were paid for on a device:
//!
//! * exporting `dlopen` took the process down before the game started — the
//!   linker calls it while it is still initialising itself;
//! * `dlsym(RTLD_NEXT, "android_dlopen_ext")` resolved back to this very
//!   function, and the forward became unbounded recursion:
//!   `Fatal signal 11 ... Cause: stack pointer is close to top of stack`.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

const ANDROID_LOG_INFO: c_int = 4;
const ANDROID_LOG_WARN: c_int = 5;

/// `dlopen` flags: resolve now, and never load anything new.
const RTLD_NOW: c_int = 2;
const RTLD_NOLOAD: c_int = 4;

type DlopenExtFn = unsafe extern "C" fn(*const c_char, c_int, *const c_void) -> *mut c_void;

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
}

/// The loader may be handed a bare soname, an absolute path in the APK's lib
/// directory, or a path inside another namespace. All of them end in this.
const TARGET: &str = "libminecraftpe.so";

/// Resolved once. Null means "not found yet" and is retried rather than cached
/// as a failure, because `dlsym` can legitimately fail very early in startup.
static REAL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// Re-entry guard: if the real `android_dlopen_ext` loads something that ends up
/// calling back into us, the inner call must pass straight through.
static IN_CALL: AtomicBool = AtomicBool::new(false);

/// Set once the target has been handled, so a second load cannot install the
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

/// The real `android_dlopen_ext`, taken from `libdl`'s own handle.
///
/// Naming the library removes the ambiguity that `RTLD_NEXT` had, and the
/// explicit self-check means a wrong resolution is refused instead of recursing.
fn real_dlopen_ext() -> Option<DlopenExtFn> {
    let cached = REAL.load(Ordering::Acquire);
    if !cached.is_null() {
        return Some(unsafe { std::mem::transmute::<*mut c_void, DlopenExtFn>(cached) });
    }

    let self_addr = android_dlopen_ext as *const () as usize;
    let libdl = unsafe { dlopen(c"libdl.so".as_ptr(), RTLD_NOW | RTLD_NOLOAD) };
    if libdl.is_null() {
        log_at(ANDROID_LOG_WARN, "libdl.so not found; refusing to forward");
        return None;
    }
    let addr = unsafe { dlsym(libdl, c"android_dlopen_ext".as_ptr()) };
    if addr.is_null() || addr as usize == self_addr {
        log_at(
            ANDROID_LOG_WARN,
            "android_dlopen_ext did not resolve to the loader's own; refusing to forward",
        );
        return None;
    }
    REAL.store(addr, Ordering::Release);
    Some(unsafe { std::mem::transmute::<*mut c_void, DlopenExtFn>(addr) })
}

// ------------------------------------------------------------------ the work

/// What happens once the game library is mapped.
///
/// The handle comes from the load that just returned, which is the most
/// reliable handle available: it is exactly the mapping the game will use.
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

// ------------------------------------------------------------- the interposer

/// ART loads the game's native library through `android_dlopen_ext`; this is the
/// one that actually fires for `System.loadLibrary`.
#[no_mangle]
pub unsafe extern "C" fn android_dlopen_ext(
    filename: *const c_char,
    flags: c_int,
    extinfo: *const c_void,
) -> *mut c_void {
    // If the real function cannot be resolved the load is refused rather than
    // mis-routed: returning null makes the caller fail loudly, which is the only
    // honest option when forwarding is impossible.
    let Some(real) = real_dlopen_ext() else {
        return std::ptr::null_mut();
    };

    // Forward first, always, and without touching the arguments: the loader's
    // work must not depend on whether we care about this library.
    let reentered = IN_CALL.swap(true, Ordering::SeqCst);
    let h = unsafe { real(filename, flags, extinfo) };
    IN_CALL.store(reentered, Ordering::SeqCst);

    if !reentered && enabled() {
        if let Some(name) = interested(filename) {
            inspect(h, &name);
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
