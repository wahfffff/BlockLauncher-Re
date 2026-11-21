//! `cargo run --example inspect -- <file.apk>`
//!
//! Reads a real APK off disk and prints what the launcher would record. This
//! exists so the format readers can be checked against files produced by real
//! toolchains, not only against the synthetic archives in the test suite.

use blre_core::{apk, catalog};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: inspect <file.apk>");
            std::process::exit(2);
        }
    };

    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        }
    };

    println!("file        {path}");
    println!("size        {} bytes", bytes.len());
    println!("sha256      {}", catalog::sha256_hex(&bytes));

    match apk::inspect(&bytes) {
        Ok(info) => {
            let m = &info.manifest;
            println!("package     {}", m.package);
            println!("version     {} ({})", m.version_name, m.version_code);
            println!("min/target  {} / {}", m.min_sdk, m.target_sdk);
            println!("label       {}", m.label.as_deref().unwrap_or("<none>"));
            println!(
                "launcher    {}",
                m.launcher_activity.as_deref().unwrap_or("<none>")
            );
            println!(
                "abis        {}",
                if info.abis.is_empty() {
                    "<none>".to_string()
                } else {
                    info.abis.join(", ")
                }
            );
        }
        Err(e) => {
            println!("error       {e}");
            std::process::exit(1);
        }
    }
}
