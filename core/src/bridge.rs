//! The one call the Android shell makes.
//!
//! The UI and this crate meet at exactly one function boundary: a request string
//! in, a reply string out. That is deliberate. Every alternative — one JNI entry
//! point per operation, or a Kotlin reimplementation of the catalog — puts the
//! decisions in a language that cannot be tested on a host machine, and the
//! interesting decisions (what identifies a build, what an import does when the
//! same APK arrives twice) are exactly the ones worth testing.
//!
//! Wire format, both directions, one line of JSON:
//!
//! ```text
//! {"cmd":"library.import","root":"/data/user/0/.../files/library","path":"/tmp/a.apk"}
//! {"ok":true,"data":{"entry":{...},"deduped":false}}
//! {"ok":false,"error":{"code":"BAD_JSON","msg":"..."}}
//! ```
//!
//! The command set is a whitelist with an explicit match, not a dynamic
//! dispatch: an unknown `cmd` is a bug in the caller and says so, rather than
//! being silently ignored.

use crate::catalog::Library;
use crate::error::Error;
use serde_json::{json, Value};
use std::path::Path;

/// Failure carried back to the shell. `code` is stable enough for the UI to
/// branch on; `msg` is for a human reading a toast.
struct Failure {
    code: &'static str,
    msg: String,
}

impl Failure {
    fn new(code: &'static str, msg: impl Into<String>) -> Self {
        Failure { code, msg: msg.into() }
    }

    fn from_core(e: Error) -> Self {
        // The core's error type already distinguishes "this file is not an APK"
        // from "this directory is unusable", and the UI wants to say which.
        let code = match &e {
            Error::NotZip(_) => "NOT_A_ZIP",
            Error::MissingEntry(_) => "MISSING_ENTRY",
            Error::BadManifest(_) => "BAD_MANIFEST",
            Error::UnusableManifest(_) => "UNUSABLE_MANIFEST",
            Error::NotFound(_) => "NOT_FOUND",
            Error::AlreadyExists(_) => "ALREADY_EXISTS",
            Error::BadMod(_) => "BAD_MOD",
            Error::Io(_) => "IO",
            Error::Corrupt(_) => "CORRUPT_INDEX",
        };
        Failure::new(code, e.to_string())
    }
}

/// Handle one request. Always returns a well-formed reply: a panic in the core
/// would take the app down with it, so the failure path is a value, not an
/// exception.
pub fn dispatch(request: &str) -> String {
    let reply = match handle(request) {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(f) => json!({ "ok": false, "error": { "code": f.code, "msg": f.msg } }),
    };
    reply.to_string()
}

fn handle(request: &str) -> std::result::Result<Value, Failure> {
    let req: Value =
        serde_json::from_str(request).map_err(|e| Failure::new("BAD_JSON", e.to_string()))?;

    let cmd = req
        .get("cmd")
        .and_then(Value::as_str)
        .ok_or_else(|| Failure::new("BAD_REQUEST", "missing \"cmd\""))?;

    match cmd {
        "core.version" => Ok(json!({ "version": crate::CORE_VERSION })),

        "library.list" => {
            let lib = open(&req)?;
            let versions: Vec<Value> = lib
                .versions()
                .iter()
                .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                .collect();
            Ok(json!({
                "root": lib.root().to_string_lossy(),
                "count": versions.len(),
                "versions": versions,
            }))
        }

        "library.import" => {
            let path = string_arg(&req, "path")?;
            let mut lib = open(&req)?;
            // Read first, import second: the user picked a file, and a file that
            // vanished between the picker and this call deserves the I/O error
            // rather than an empty-library message.
            let bytes = std::fs::read(&path).map_err(|e| {
                Failure::new("IO", format!("cannot read {path}: {e}"))
            })?;
            let out = lib.import(&bytes).map_err(Failure::from_core)?;
            Ok(json!({
                "deduped": out.deduped,
                "entry": out.entry,
                "count": lib.versions().len(),
            }))
        }

        "library.remove" => {
            let id = string_arg(&req, "id")?;
            let mut lib = open(&req)?;
            let removed = lib.remove(&id).map_err(Failure::from_core)?;
            Ok(json!({ "removed": removed, "count": lib.versions().len() }))
        }

        other => Err(Failure::new(
            "UNKNOWN_COMMAND",
            format!("unknown command: {other}"),
        )),
    }
}

fn string_arg(req: &Value, key: &str) -> std::result::Result<String, Failure> {
    req.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Failure::new("BAD_REQUEST", format!("missing \"{key}\"")))
}

fn open(req: &Value) -> std::result::Result<Library, Failure> {
    let root = string_arg(req, "root")?;
    Library::open(Path::new(&root)).map_err(Failure::from_core)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let mut p = std::env::temp_dir();
            let unique = format!(
                "blre-bridge-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            p.push(unique);
            std::fs::create_dir_all(&p).expect("temp dir");
            TempDir(p)
        }

        fn s(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn parse(reply: &str) -> Value {
        serde_json::from_str(reply).expect("reply is JSON")
    }

    fn code(reply: &str) -> String {
        parse(reply)["error"]["code"].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn core_version_answers() {
        let v = parse(&dispatch(r#"{"cmd":"core.version"}"#));
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"]["version"], crate::CORE_VERSION);
    }

    #[test]
    fn bad_json_is_named() {
        assert_eq!(code(&dispatch("{not json")), "BAD_JSON");
    }

    #[test]
    fn unknown_command_is_named() {
        assert_eq!(
            code(&dispatch(r#"{"cmd":"library.explode"}"#)),
            "UNKNOWN_COMMAND"
        );
    }

    #[test]
    fn missing_root_is_a_bad_request() {
        assert_eq!(code(&dispatch(r#"{"cmd":"library.list"}"#)), "BAD_REQUEST");
    }

    #[test]
    fn library_list_starts_empty() {
        let t = TempDir::new("empty");
        let reply = dispatch(&format!(
            r#"{{"cmd":"library.list","root":{}}}"#,
            serde_json::to_string(&t.s()).unwrap()
        ));
        let v = parse(&reply);
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"]["count"], 0);
        assert_eq!(v["data"]["versions"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn import_of_a_non_apk_fails_without_adding() {
        let t = TempDir::new("notapk");
        let apk = t.0.join("not-an.apk");
        std::fs::write(&apk, b"definitely not a zip").unwrap();

        let reply = dispatch(&format!(
            r#"{{"cmd":"library.import","root":{},"path":{}}}"#,
            serde_json::to_string(&t.s()).unwrap(),
            serde_json::to_string(&apk.to_string_lossy()).unwrap()
        ));
        assert_eq!(code(&reply), "NOT_A_ZIP");
        assert_eq!(parse(&reply)["ok"], false);
    }

    #[test]
    fn import_of_a_missing_file_says_which_file() {
        let t = TempDir::new("missing");
        let reply = dispatch(&format!(
            r#"{{"cmd":"library.import","root":{},"path":"/nonexistent/x.apk"}}"#,
            serde_json::to_string(&t.s()).unwrap()
        ));
        assert_eq!(code(&reply), "IO");
        assert!(
            reply.contains("/nonexistent/x.apk"),
            "the message should name the path: {reply}"
        );
    }

    #[test]
    fn remove_of_an_unknown_id_is_an_error_not_a_silent_success() {
        let t = TempDir::new("remove");
        let reply = dispatch(&format!(
            r#"{{"cmd":"library.remove","root":{},"id":"nope"}}"#,
            serde_json::to_string(&t.s()).unwrap()
        ));
        assert_eq!(parse(&reply)["ok"], false);
    }

    #[test]
    fn every_reply_is_one_line() {
        // The JNI side turns a reply into a Java string; a newline would still
        // survive, but a caller that line-splits would break, so assert it here.
        for req in [
            r#"{"cmd":"core.version"}"#,
            "{broken",
            r#"{"cmd":"nope"}"#,
        ] {
            assert!(!dispatch(req).contains('\n'), "reply has a newline: {req}");
        }
    }
}
