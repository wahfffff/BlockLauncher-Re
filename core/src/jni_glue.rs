//! The JNI seam between the Kotlin shell and [`crate::bridge`].
//!
//! One exported function, because one is enough: everything the UI can ask for
//! is a JSON command handed to `dispatch`. Multiple JNI entry points would mean
//! re-deriving Java strings, byte arrays and error objects per call, and each of
//! those is a place to get the FFI plumbing wrong — for no gain, since the
//! interesting work is on the other side of the string.
//!
//! This module is empty off-device. `cargo test` on a host machine must not need
//! a JVM, and the tests that matter (the bridge) run there.

#[cfg(target_os = "android")]
mod imp {
    use crate::bridge::dispatch;
    use jni::objects::{JObject, JString};
    use jni::sys::jstring;
    use jni::JNIEnv;

    /// `dev.blre.launcher.NativeBridge.call`
    ///
    /// Signature note: `NativeBridge` is a Kotlin `object`, so `call` is an
    /// instance method and the second parameter is `this`, not the class. An
    /// `extern "C"` declaration that took `JClass` would still link and would
    /// still appear to work for the argument we use — the mistake would only
    /// surface on a signature-checked JVM.
    #[no_mangle]
    pub extern "system" fn Java_dev_blre_launcher_NativeBridge_call<'local>(
        mut env: JNIEnv<'local>,
        _this: JObject<'local>,
        request: JString<'local>,
    ) -> jstring {
        let text = match env.get_string(&request) {
            Ok(s) => s.to_string_lossy().into_owned(),
            // A Java string that cannot be decoded is a caller bug; answer in the
            // same envelope as everything else so the UI has one error path.
            Err(e) => return answer(&mut env, &error_reply("JNI", &e.to_string())),
        };

        // The second safety net. `run_detached` keeps script stack overflows and
        // panics inside a thread it owns; this keeps *any other* panic — a bug in
        // the catalog, an index that trips an assertion — from crossing the FFI
        // boundary and aborting the process. `extern "system"` would turn an
        // escaping unwind into an abort, so catching here is the only place it can
        // be turned into the reply the Kotlin side already knows how to show.
        //
        // Requires `panic = "unwind"`; with `abort` this is dead code and a panic
        // kills the app, which is what it used to do.
        let reply = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch(&text)))
        {
            Ok(reply) => reply,
            Err(_) => error_reply("PANIC", "the core panicked; the launcher is still running"),
        };
        answer(&mut env, &reply)
    }

    fn error_reply(code: &str, msg: &str) -> String {
        serde_json::json!({ "ok": false, "error": { "code": code, "msg": msg } }).to_string()
    }

    fn answer<'local>(env: &mut JNIEnv<'local>, text: &str) -> jstring {
        // Returning null on allocation failure is legal for a `String` return; the
        // Kotlin side treats a null reply as an unreachable core rather than
        // crashing the app.
        match env.new_string(text) {
            Ok(s) => s.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    }
}
