package dev.blre.launcher

/**
 * The only door into the Rust core.
 *
 * The library name and the symbol name are both fixed by this file:
 * `cargo ndk` writes `libblre_core.so` (the crate's `[lib] name`) into
 * `jniLibs/<abi>/`, and the exported symbol is
 * `Java_dev_blre_launcher_NativeBridge_call` — derived from this package and
 * this object. Renaming either one here without renaming it in `core/src/jni_glue.rs`
 * produces an `UnsatisfiedLinkError` at first call, not at build time.
 */
object NativeBridge {
    init {
        System.loadLibrary("blre_core")
    }

    /** Returns null if the core could not allocate the reply. */
    external fun call(request: String): String?
}
