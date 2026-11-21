//! Print what `mods::scan` makes of a real folder.
//!
//! Used during development against the sample `.mod` files this format was
//! derived from; nothing in the launcher depends on it.
//!
//! `cargo run --example modscan -- <dir>`

use blre_core::mods;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let entries = match mods::scan(std::path::Path::new(&dir)) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    println!("{:<28} {:<7} {:<14} {}", "file", "kind", "author", "status");
    println!("{}", "-".repeat(92));
    for e in &entries {
        println!(
            "{:<28} {:<7} {:<14} {}",
            e.file_name,
            match e.kind {
                mods::Kind::Native => "native",
                mods::Kind::Legacy => "legacy",
            },
            if e.author.is_empty() { "-" } else { &e.author },
            if e.usable { "usable" } else { &e.reason }
        );
    }

    let usable = entries.iter().filter(|e| e.usable).count();
    println!(
        "\n{} entries, {} usable, {} describable only",
        entries.len(),
        usable,
        entries.len() - usable
    );
}
