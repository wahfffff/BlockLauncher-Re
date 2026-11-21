//! End-to-end tests for the script runtime.
//!
//! These are the specification for what a ModPE script can rely on. They run the
//! real engine against a recording host, so a change that breaks `Level.setTile`
//! fails here rather than on a phone.

use super::*;
use crate::script::API;
use boa_engine::JsString;
use std::collections::HashMap;

#[derive(Default)]
struct Recording {
    tiles: HashMap<(i32, i32, i32), i32>,
    pos: (f64, f64, f64),
    data: HashMap<String, String>,
    log: Vec<String>,
}

impl Host for Recording {
    fn log(&mut self, level: &str, message: &str) {
        self.log.push(format!("[{level}] {message}"));
    }
    fn minecraft_version(&mut self) -> std::result::Result<String, String> {
        Ok("0.14.3".into())
    }
    fn get_tile(&mut self, x: i32, y: i32, z: i32) -> std::result::Result<i32, String> {
        Ok(*self.tiles.get(&(x, y, z)).unwrap_or(&0))
    }
    fn set_tile(&mut self, x: i32, y: i32, z: i32, id: i32, _meta: i32) -> std::result::Result<(), String> {
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
        Ok(2)
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

/// Every name in [`API`] must exist on the global object. This is the test that
/// stops the published table and the binding from drifting apart.
#[test]
fn every_advertised_name_is_installed() {
    let mut ctx = Context::default();
    install(&mut ctx).expect("globals install");

    for (name, _) in API {
        let mut parts = name.split('.');
        let head = parts.next().unwrap();
        let global = ctx.global_object();
        let mut cur = global.get(JsString::from(head), &mut ctx).unwrap_or_default();
        assert!(!cur.is_undefined(), "{name}: {head} is not defined");

        for part in parts {
            let Some(obj) = cur.as_object() else {
                panic!("{name}: {part} has no object to live on");
            };
            cur = obj.get(JsString::from(part), &mut ctx).unwrap_or_default();
            assert!(!cur.is_undefined(), "{name}: {part} is not defined");
        }
    }
}

#[test]
fn a_script_can_read_the_player_position() {
    let mut host = Recording { pos: (1.5, 64.0, -3.25), ..Default::default() };
    host.set_player_position(1.5, 64.0, -3.25).unwrap();
    let report = run("var seen = Player.getX() + Player.getZ();", Box::new(host)).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.messages);
}

#[test]
fn a_script_can_write_a_tile() {
    let report = run("Level.setTile(1, 2, 3, 41, 0);", Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.messages);
}

#[test]
fn modpe_data_round_trips_through_the_host() {
    let script = r#"
        ModPE.saveData("k", "v");
        var back = ModPE.readData("k");
        if (back !== "v") throw new Error("round trip failed: " + back);
    "#;
    let report = run(script, Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.messages);
}

#[test]
fn a_missing_data_key_reads_as_null_not_empty_string() {
    // "" and "absent" must stay distinguishable or a script cannot tell a stored
    // empty value from one it never wrote.
    let script = r#"if (ModPE.readData("nope") !== null) throw new Error("not null");"#;
    let report = run(script, Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.messages);
}

#[test]
fn print_reaches_the_host_log() {
    let report = run(r#"print("hello", 42);"#, Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.messages);
}

#[test]
fn a_syntax_error_is_a_report_not_a_panic() {
    let report = run("function (", Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(report.ran, 0);
    assert!(!report.messages.is_empty());
}

#[test]
fn an_unwired_call_throws_with_a_message_that_says_so() {
    let report = run("Level.explode(0, 1, 2, 4);", Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 1, "a call with no host behind it must fail");
    assert!(
        report.messages.iter().any(|m| m.contains("not wired")),
        "{:?}",
        report.messages
    );
}

#[test]
fn the_new_level_hook_runs_after_load() {
    let script = r#"function newLevel() { Level.setTile(9, 9, 9, 7, 0); }"#;
    let report = run(script, Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.messages);
}

#[test]
fn entity_get_all_returns_one_handle_per_entity() {
    let script = r#"if (Entity.getAll().length !== 2) throw new Error("wrong count");"#;
    let report = run(script, Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.messages);
}

#[test]
fn client_message_is_routed_to_the_chat_channel() {
    let script = r#"clientMessage("hi");"#;
    let report = run(script, Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.messages);
}

#[test]
fn a_script_with_no_hooks_still_reports_success() {
    let report = run("var x = 1 + 1;", Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 0);
    assert_eq!(report.ran, 1);
}

// ------------------------------------------------------------ execution budget

#[test]
fn an_endless_loop_is_stopped_rather_than_waited_on() {
    let report = run_with("while (true) {}", Box::new(Recording::default()), Limits::tiny())
        .expect("a halted script is still a report");
    assert!(report.halted, "{:?}", report.messages);
    assert_eq!(report.failed, 1);
    assert_eq!(report.ran, 0);
    assert!(
        report.messages.iter().any(|m| m.contains("loop budget")),
        "{:?}",
        report.messages
    );
}

#[test]
fn an_endless_loop_inside_a_hook_is_stopped_too() {
    // The budget applies to hook calls, not only to the top-level script: a mod
    // whose `newLevel` spins is the likelier shape of this bug.
    let script = "function newLevel() { while (true) {} }";
    let report = run_with(script, Box::new(Recording::default()), Limits::tiny()).unwrap();
    assert!(report.halted, "{:?}", report.messages);
    assert!(
        report
            .messages
            .iter()
            .any(|m| m.contains("newLevel") && m.contains("loop budget")),
        "{:?}",
        report.messages
    );
}

#[test]
fn runaway_recursion_hits_the_recursion_ceiling() {
    let script = "function f(n) { return f(n + 1); } f(0);";
    let report = run_with(script, Box::new(Recording::default()), Limits::tiny()).unwrap();
    assert!(report.halted, "{:?}", report.messages);
    assert!(
        report.messages.iter().any(|m| m.contains("recursion budget")),
        "{:?}",
        report.messages
    );
}

#[test]
fn a_normal_script_fits_inside_the_default_budget() {
    // The default must not stop honest work: a thousand iterations is already
    // more looping than a mod tick does.
    let script = "var s = 0; for (var i = 0; i < 1000; i++) { s += i; } \
                  if (s !== 499500) throw new Error('bad sum: ' + s);";
    let report = run(script, Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.messages);
    assert!(!report.halted, "{:?}", report.messages);
}

/// A generated script can present the parser with a left-leaning tree thousands
/// of nodes deep, which is what a recursive walk needs a lot of stack for.
///
/// The shape matters and was measured: bracket nesting (max 21 across the real
/// scripts) and statement count are both innocent — 8000 flat `var` statements
/// parse in 1 MiB — while a long `a + a + a …` chain or a long `else if` chain
/// overflows it. 2000 terms overflow 1 MiB and fit comfortably in
/// [`STACK_BYTES`]; before the runtime had its own thread this aborted the app,
/// which is what "importing a .js closes the launcher" was.
#[test]
fn a_long_operator_chain_parses_off_the_caller_thread() {
    let source = format!("var x = 0{};\n", "+1".repeat(2000));

    let report = run_detached(source, Limits::default()).expect("runs on its own stack");
    assert_eq!(report.failed, 0, "{:?}", report.messages);
    assert!(!report.halted, "{:?}", report.messages);
}

/// The same shape through the ordinary call path, which is what the bridge used
/// to do. This is here to make the contrast explicit: `run` is the same-thread
/// entry point and is fine for small inputs, so the test only asserts that the
/// *detached* path is what the bridge uses — mixing them up is the bug.
#[test]
fn the_bridge_runs_scripts_through_the_detached_entry_point() {
    let source = crate::bridge::dispatch(r#"{"cmd":"scripts.check","source":"var a = 1;"}"#);
    let v: serde_json::Value = serde_json::from_str(&source).expect("a reply");
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["ran"], 1);
}

/// A script reaching into the game's Java objects is the normal case for
/// BlockLauncher mods, so the error has to say what is missing rather than
/// "com is not defined".
#[test]
fn touching_the_java_bridge_explains_what_is_missing() {
    let script = r#"var ctx = com.mojang.minecraftpe.MainActivity.currentMainActivity.get();"#;
    let report = run(script, Box::new(Recording::default())).unwrap();

    assert_eq!(report.failed, 1);
    let message = report.messages.join(" ");
    assert!(message.contains("Java bridge"), "{message}");
    assert!(message.contains("com"), "{message}");
}

/// Rhino can subclass a Java class in place; ECMAScript cannot, and the engine's
/// own complaint (`expected one of ',' or ')', got '{'`) names the punctuation
/// instead of the feature. The message is replaced, not the verdict: the script
/// still counts as failed.
#[test]
fn a_java_adapter_is_explained_rather_than_reported_as_punctuation() {
    let script = r#"
        var d = new android.app.AlertDialog.Builder(ctx).setView(sv)
            .setNegativeButton("ok", new android.content.DialogInterface.OnClickListener() {
                onClick: function (mp) { }
            });
    "#;

    let report = run(script, Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 1);

    let message = report.messages.join(" ");
    assert!(message.contains("subclasses a Java class"), "{message}");
    assert!(message.contains("Rhino"), "{message}");
    assert!(!message.contains("expected one of"), "{message}");
}

/// The detector only ever rewrites the wording of a failure the engine already
/// reported, so text that merely looks like a JavaAdapter cannot cost a working
/// script anything.
#[test]
fn javascript_that_mentions_a_java_adapter_still_runs() {
    let script = r#"var s = "new a.b(){}"; if (s.length === 0) { throw new Error("no"); }"#;
    let report = run(script, Box::new(Recording::default())).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.messages);
}

/// The detector must not fire on ordinary JavaScript, including a call whose
/// argument is an object literal — the shape it looks for ends in `){`.
#[test]
fn the_java_adapter_detector_ignores_ordinary_code() {
    for source in [
        "var x = new Foo();",
        "var x = new Foo(1, 2);",
        "var x = new java.util.ArrayList();",
        "var x = {a: 1};",
        "var x = f(function () { return 1; });",
        "var x = new Foo().bar({a: 1});",
    ] {
        assert!(
            !looks_like_java_adapter(source),
            "false positive on: {source}"
        );
    }
}

#[test]
fn the_engine_still_words_limits_the_way_we_detect_them() {
    // Detection is textual because the engine does not export the limit payload.
    // That is only safe while this test passes; an upgrade that renames the
    // message turns "stopped by the loop budget" into "unknown error", which is
    // still reported, but this test is what makes that deliberate rather than a
    // surprise.
    let mut ctx = Context::default();
    install(&mut ctx).expect("globals install");
    ctx.runtime_limits_mut().set_loop_iteration_limit(500);
    let e = ctx.eval(Source::from_bytes(b"while (true) {}")).expect_err("must trip");
    let text = e.to_string();
    assert_eq!(limit_hit(&text), Some("loop"), "engine wording changed: {text}");

    let mut ctx = Context::default();
    ctx.runtime_limits_mut().set_recursion_limit(16);
    let e = ctx
        .eval(Source::from_bytes(b"function f() { return f(); } f();"))
        .expect_err("must trip");
    let text = e.to_string();
    assert_eq!(limit_hit(&text), Some("recursion"), "engine wording changed: {text}");
}
