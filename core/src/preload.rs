//! The half of the runtime that has to exist before the game does.
//!
//! Android offers no supported way into another app's process, so the only entry
//! point is the dynamic linker: `wrap.<package>` set to `LD_PRELOAD=<library>`
//! makes the linker map this library first, in the target's own process, under
//! the target's own uid and SELinux domain. Verified on MCPE 0.14.3 / Android 16:
//! the four mappings of this library appear in `/proc/<pid>/maps` and the process
//! runs normally.
//!
//! `.init_array` runs before the game exists, so the game library is not mapped
//! yet and there is nothing to resolve. Something has to run later, when it is.
//!
//! # Why this watches `/proc/self/maps` instead of interposing the loader
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
//!    namespace and the process aborted (`SIGABRT`).
//!
//! The watch has a fourth, quieter failure of its own, and it is the one that
//! settled the design: a library mapped in the app's classloader namespace is
//! invisible to `dlopen(..., RTLD_NOLOAD)` from the default namespace, so the
//! handle came back null even though the library was plainly there. Namespaces
//! cannot be crossed from inside the namespace — so this file does not cross one.
//! It reads the symbols out of the file the mapping names (see `inject`) and adds
//! the load address, which needs no handle.
//!
//! The trade is stated plainly: hooks are installed when the library appears —
//! milliseconds after it is loaded, **not before the first instruction of the game
//! runs**. A hook target called inside the game's own `JNI_OnLoad` would be missed.
//! Nothing in the ModPE surface is reachable that early, but that is a real cost,
//! not zero.

use std::ffi::{c_char, c_int, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::inject;

const ANDROID_LOG_INFO: c_int = 4;
const ANDROID_LOG_WARN: c_int = 5;

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// The library the hooks live in.
const TARGET: &str = "libminecraftpe.so";

/// How long to keep watching for it.
const WATCH_SECS: u32 = 30;
const POLL_MS: u32 = 10;

/// Enough bytes to compare the prologue and to plan an 8-byte patch.
const WINDOW: usize = 16;

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

/// Bytes of this process's address space, read the way the kernel will report a
/// failure instead of raising one.
///
/// Dereferencing `addr` directly would fault on an unmapped page, and a fault here
/// is the game's crash, not this library's. `pread` on `/proc/self/mem` returns
/// `EIO` for an address that is not mapped, which turns the same mistake into a
/// log line.
fn read_mem(addr: u64, len: usize) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open("/proc/self/mem").ok()?;
    f.seek(SeekFrom::Start(addr)).ok()?;
    let mut buf = vec![0u8; len];
    let mut got = 0usize;
    while got < len {
        match f.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(_) => return None,
        }
    }
    buf.truncate(got);
    (!buf.is_empty()).then_some(buf)
}

/// What happens once the game library is mapped.
fn on_game_loaded(mapping: &inject::Mapping) {
    log(&format!("game library at {:#x}: {}", mapping.base, mapping.path));

    let Ok(file) = std::fs::read(&mapping.path) else {
        log_at(ANDROID_LOG_WARN, "the mapped file could not be read");
        return;
    };
    let Ok((header, symbols)) = crate::elf::parse(&file) else {
        log_at(ANDROID_LOG_WARN, "the mapped file is not an ELF this build understands");
        return;
    };
    log(&format!(
        "  {}: {} symbols",
        header.arch_name(),
        header.symbol_count
    ));

    let mut resolved = 0usize;
    let mut verified = 0usize;
    let mut hookable = 0usize;
    let mut missing: Vec<&str> = Vec::new();

    for point in crate::hook::POINTS {
        let hit = point
            .candidates
            .iter()
            .find_map(|name| inject::symbol(&symbols, name).map(|s| (*name, s)));

        let Some((name, sym)) = hit else {
            missing.push(point.id);
            log_at(ANDROID_LOG_WARN, &format!("  {} MISSING", point.id));
            continue;
        };
        resolved += 1;

        let addr = inject::at(mapping.base, sym);
        let size = u32::try_from(sym.size).ok();

        // The load address is a guess until the file says otherwise: the bytes at
        // `base + st_value` must be the bytes at `st_value` in the file.
        let file_bytes: Vec<u8> = file
            .get(sym.value as usize..(sym.value as usize).saturating_add(WINDOW))
            .unwrap_or(&[])
            .to_vec();
        let mem_bytes = read_mem(addr, WINDOW).unwrap_or_default();
        let same = !file_bytes.is_empty()
            && mem_bytes.len() >= file_bytes.len().min(4)
            && mem_bytes[..file_bytes.len().min(4)] == file_bytes[..file_bytes.len().min(4)];
        if !same {
            // Patching on an unverified base is how a hook lands in the wrong
            // function and the game dies somewhere unrelated.
            log_at(
                ANDROID_LOG_WARN,
                &format!("  {:<18} {addr:#x} DIFFERS from the file; refusing", point.id),
            );
            continue;
        }
        verified += 1;

        let verdict = match crate::thumb::plan(&file_bytes, size) {
            Ok(_) => {
                hookable += 1;
                "hookable"
            }
            Err(why) => {
                log_at(
                    ANDROID_LOG_WARN,
                    &format!("  {:<18} {name} @ {addr:#x} not hookable: {why}", point.id),
                );
                continue;
            }
        };

        log(&format!(
            "  {:<18} {addr:#x} bytes {} {verdict}",
            point.id,
            if same { "match" } else { "DIFFER" }
        ));
    }

    log(&format!(
        "resolved {resolved}/{}, verified {verified}, hookable {hookable}{}",
        crate::hook::POINTS.len(),
        if missing.is_empty() {
            String::new()
        } else {
            format!("; missing: {}", missing.join(", "))
        }
    ));

    if verified < resolved {
        log_at(
            ANDROID_LOG_WARN,
            "a resolved address did not match the file; the load base is wrong and nothing must be patched",
        );
    }
}

/// Run the post-load work, but never unwind across a foreign frame: a panic here
/// would be undefined behaviour rather than a diagnosable crash.
fn inspect(mapping: &inject::Mapping) {
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        on_game_loaded(mapping);
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
        let found = std::fs::read_to_string("/proc/self/maps")
            .ok()
            .and_then(|maps| inject::find(&maps, TARGET));
        if let Some(mapping) = found {
            inspect(&mapping);
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
