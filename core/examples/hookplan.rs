//! Resolve the hook plan for a real game APK, without a device.
//!
//! ```sh
//! cargo run --example hookplan -- /path/to/minecraft.apk
//! ```
//!
//! Exits non-zero when a required point is missing, so this can be used as a
//! check against a new game build in the same way `inspect` is used against a
//! new APK.

use std::process::ExitCode;

use blre_core::{apk, hook};

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: hookplan <game.apk>");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let archive = match apk::Apk::open(&bytes) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };

    let abis = apk::abis(&archive);
    println!("apk       {path}");
    println!("abis      {}", abis.join(", "));

    let mut any = false;
    let mut usable = false;
    for abi in &abis {
        let name = format!("lib/{abi}/libminecraftpe.so");
        let Some(entry) = archive.entry(&name) else {
            println!("\n{abi}: no {name}");
            continue;
        };
        let so = match archive.read(entry) {
            Ok(v) => v,
            Err(e) => {
                println!("\n{abi}: {e}");
                continue;
            }
        };
        any = true;
        match hook::plan(&so) {
            Ok(plan) => {
                usable |= plan.usable();
                println!("\n{abi}: {}", plan.summary());
                println!("  symbols   {}", plan.symbol_count);
                for r in &plan.resolved {
                    println!("  ok        {:<20} {}", r.id, r.symbol);
                }
                for m in &plan.missing {
                    let kind = if m.required { "MISSING " } else { "optional" };
                    println!("  {kind:<9} {:<20} wanted: {}", m.id, m.tried.join(" | "));
                }
            }
            Err(e) => println!("\n{abi}: {e}"),
        }
    }

    if !any {
        eprintln!("\nno libminecraftpe.so in any ABI — this is not a game APK");
        return ExitCode::from(1);
    }
    if usable {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
