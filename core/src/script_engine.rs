//! Binding [`crate::script::API`] to a JavaScript engine.
//!
//! The engine is Boa: pure Rust, no C, so the Android build needs no extra NDK
//! cross-compile step beyond the one it already runs. The cost is speed, and
//! that is the right trade here — a mod script runs a few hundred statements a
//! tick, not a benchmark.
//!
//! Every global in the table is installed the same way, from one list, so the
//! table in `script.rs`, the docs and the code cannot drift apart.

use crate::error::Error;
use crate::script::{enter, with_host as host_call, Host, Report, API};
use boa_engine::{
    js_string, object::builtins::JsArray, object::ObjectInitializer, property::Attribute, Context,
    JsError, JsNativeError, JsValue, NativeFunction, Source,
};

fn js_err(message: impl Into<String>) -> JsError {
    JsNativeError::error().with_message(message.into()).into()
}

fn num(args: &[JsValue], i: usize, name: &str) -> std::result::Result<f64, JsError> {
    args.get(i)
        .and_then(JsValue::as_number)
        .ok_or_else(|| js_err(format!("{name}: argument {} must be a number", i + 1)))
}

fn int(args: &[JsValue], i: usize, name: &str) -> std::result::Result<i32, JsError> {
    let v = num(args, i, name)?;
    if !v.is_finite() {
        return Err(js_err(format!("{name}: argument {} is not a finite number", i + 1)));
    }
    Ok(v as i32)
}

fn text(args: &[JsValue], i: usize, name: &str) -> std::result::Result<String, JsError> {
    args.get(i)
        .and_then(JsValue::as_string)
        .map(|s| s.to_std_string_escaped())
        .ok_or_else(|| js_err(format!("{name}: argument {} must be a string", i + 1)))
}

// ---------------------------------------------------------------- free functions

fn f_print(_: &JsValue, args: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    let msg = args
        .iter()
        .map(|v| v.display().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let _ = host_call(|h| {
        h.log("script", &msg);
        Ok(())
    });
    Ok(JsValue::undefined())
}

fn f_client_message(_: &JsValue, args: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    let msg = text(args, 0, "clientMessage")?;
    host_call(|h| {
        h.log("chat", &msg);
        Ok(())
    })
    .map_err(js_err)?;
    Ok(JsValue::undefined())
}

// ---------------------------------------------------------------------- Level

fn level_get_tile(_: &JsValue, args: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    let (x, y, z) = (int(args, 0, "Level.getTile")?, int(args, 1, "Level.getTile")?, int(args, 2, "Level.getTile")?);
    let id = host_call(|h| h.get_tile(x, y, z)).map_err(js_err)?;
    Ok(JsValue::from(id as f64))
}

fn level_set_tile(_: &JsValue, args: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    let x = int(args, 0, "Level.setTile")?;
    let y = int(args, 1, "Level.setTile")?;
    let z = int(args, 2, "Level.setTile")?;
    let id = int(args, 3, "Level.setTile")?;
    let meta = args.get(4).and_then(JsValue::as_number).unwrap_or(0.0) as i32;
    host_call(|h| h.set_tile(x, y, z, id, meta)).map_err(js_err)?;
    Ok(JsValue::undefined())
}

fn level_version(_: &JsValue, _: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    let v = host_call(|h| h.minecraft_version()).map_err(js_err)?;
    Ok(JsValue::from(js_string!(v)))
}

fn level_explode(_: &JsValue, args: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    // Deliberately reachable but answered by the host: destruction is a real
    // operation, and a launcher that faked it would be lying to the script about
    // the world it just changed.
    let x = num(args, 0, "Level.explode")?;
    let y = num(args, 1, "Level.explode")?;
    let z = num(args, 2, "Level.explode")?;
    let power = args.get(3).and_then(JsValue::as_number).unwrap_or(1.0);
    host_call(|_| Err(format!("Level.explode({x}, {y}, {z}, {power}) is not wired to a game yet")))
        .map_err(js_err)?;
    Ok(JsValue::undefined())
}

// --------------------------------------------------------------------- Player

fn coord(axis: usize) -> std::result::Result<JsValue, JsError> {
    let pos = host_call(|h| h.player_position()).map_err(js_err)?;
    let v = match axis {
        0 => pos.0,
        1 => pos.1,
        _ => pos.2,
    };
    Ok(JsValue::from(v))
}

fn player_x(_: &JsValue, _: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    coord(0)
}

fn player_y(_: &JsValue, _: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    coord(1)
}

fn player_z(_: &JsValue, _: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    coord(2)
}

fn player_set_position(_: &JsValue, args: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    let x = num(args, 0, "Player.setPosition")?;
    let y = num(args, 1, "Player.setPosition")?;
    let z = num(args, 2, "Player.setPosition")?;
    host_call(|h| h.set_player_position(x, y, z)).map_err(js_err)?;
    Ok(JsValue::undefined())
}

// --------------------------------------------------------------------- Entity

fn entity_get_all(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> std::result::Result<JsValue, JsError> {
    let n = host_call(|h| h.entity_count()).map_err(js_err)?;
    let ids: Vec<JsValue> = (0..n).map(|i| JsValue::from(i as f64)).collect();
    let array = JsArray::from_iter(ids, ctx);
    Ok(array.into())
}

// ---------------------------------------------------------------------- ModPE

fn modpe_read(_: &JsValue, args: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    let key = text(args, 0, "ModPE.readData")?;
    let value = host_call(|h| h.data_get(&key)).map_err(js_err)?;
    Ok(match value {
        // `readData` on a missing key returns null in the launcher's semantics;
        // returning "" would be indistinguishable from a stored empty string.
        None => JsValue::null(),
        Some(v) => JsValue::from(js_string!(v)),
    })
}

fn modpe_save(_: &JsValue, args: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    let key = text(args, 0, "ModPE.saveData")?;
    let value = text(args, 1, "ModPE.saveData")?;
    host_call(|h| h.data_set(&key, Some(&value))).map_err(js_err)?;
    Ok(JsValue::undefined())
}

fn modpe_remove(_: &JsValue, args: &[JsValue], _: &mut Context) -> std::result::Result<JsValue, JsError> {
    let key = text(args, 0, "ModPE.removeData")?;
    host_call(|h| h.data_set(&key, None)).map_err(js_err)?;
    Ok(JsValue::undefined())
}

/// Build the global object graph described by [`API`].
fn install(ctx: &mut Context) -> std::result::Result<(), JsError> {
    let free = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(f_print), js_string!("print"), 1)
        .function(NativeFunction::from_fn_ptr(f_client_message), js_string!("clientMessage"), 1)
        .build();
    let print = free.get(js_string!("print"), ctx)?;
    ctx.register_global_property(js_string!("print"), print, Attribute::all())?;
    let client_message = free.get(js_string!("clientMessage"), ctx)?;
    ctx.register_global_property(js_string!("clientMessage"), client_message, Attribute::all())?;

    let level = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(level_get_tile), js_string!("getTile"), 3)
        .function(NativeFunction::from_fn_ptr(level_set_tile), js_string!("setTile"), 5)
        .function(NativeFunction::from_fn_ptr(level_version), js_string!("getMinecraftVersion"), 0)
        .function(NativeFunction::from_fn_ptr(level_explode), js_string!("explode"), 4)
        .build();
    ctx.register_global_property(js_string!("Level"), level, Attribute::all())?;

    let player = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(player_x), js_string!("getX"), 0)
        .function(NativeFunction::from_fn_ptr(player_y), js_string!("getY"), 0)
        .function(NativeFunction::from_fn_ptr(player_z), js_string!("getZ"), 0)
        .function(NativeFunction::from_fn_ptr(player_set_position), js_string!("setPosition"), 3)
        .build();
    ctx.register_global_property(js_string!("Player"), player, Attribute::all())?;

    let entity = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(entity_get_all), js_string!("getAll"), 0)
        .build();
    ctx.register_global_property(js_string!("Entity"), entity, Attribute::all())?;

    let modpe = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(modpe_read), js_string!("readData"), 1)
        .function(NativeFunction::from_fn_ptr(modpe_save), js_string!("saveData"), 2)
        .function(NativeFunction::from_fn_ptr(modpe_remove), js_string!("removeData"), 1)
        .build();
    ctx.register_global_property(js_string!("ModPE"), modpe, Attribute::all())?;

    Ok(())
}

/// Call `name` on the global object if a script defined it.
fn call_hook(ctx: &mut Context, name: &str) -> std::result::Result<bool, JsError> {
    let global = ctx.global_object();
    let value = global.get(js_string!(name), ctx)?;
    if !value.is_callable() {
        return Ok(false);
    }
    let callable = value.as_callable().expect("checked callable");
    callable.call(&JsValue::undefined(), &[], ctx)?;
    Ok(true)
}

/// Run one script against `host`, then call `newLevel` if the script defined it.
pub fn run(source: &str, host: Box<dyn Host>) -> crate::Result<Report> {
    let _guard = enter(host);
    let mut ctx = Context::default();
    install(&mut ctx).map_err(|e| Error::BadScript(format!("cannot build the script globals: {e}")))?;

    let mut report = Report::default();
    match ctx.eval(Source::from_bytes(source)) {
        Ok(_) => report.ran += 1,
        Err(e) => {
            report.failed += 1;
            report.messages.push(format!("{e}"));
            // A script that failed to parse cannot have defined hooks; running
            // them would attribute its errors to the wrong place.
            return Ok(report);
        }
    }

    if let Err(e) = call_hook(&mut ctx, "newLevel") {
        report.messages.push(format!("newLevel: {e}"));
    }
    if let Err(e) = call_hook(&mut ctx, "selectLevelHook") {
        report.messages.push(format!("selectLevelHook: {e}"));
    }

    Ok(report)
}

/// Advance one tick: call `modTick(ticks)` when a script defined it.
pub fn tick(ctx: &mut Context, ticks: u64) -> std::result::Result<bool, JsError> {
    let global = ctx.global_object();
    let value = global.get(js_string!("modTick"), ctx)?;
    if !value.is_callable() {
        return Ok(false);
    }
    let callable = value.as_callable().expect("checked callable");
    callable.call(&JsValue::undefined(), &[JsValue::from(ticks as f64)], ctx)?;
    Ok(true)
}

/// The names [`API`] claims to provide. Used by a test to prove the table and the
/// installed objects agree.
pub fn provided() -> Vec<String> {
    API.iter().map(|(name, _)| (*name).to_string()).collect()
}

#[cfg(test)]
#[path = "script_tests.rs"]
mod script_tests;
