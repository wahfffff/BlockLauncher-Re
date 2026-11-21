//! Run one real file through the core, in isolation.
//!
//! Exists so a crash in one archive or script cannot hide the result of the
//! next: the caller runs this once per file and reads the exit code.
//!
//!   loadall pack   <file.zip>
//!   loadall script <file.js>
//!
//! Exit codes: 0 ok, 3 the core refused it, 4 it panicked, 5 not UTF-8.
//!
//! Scripts run on a thread with a 1 MiB stack, which is what an Android JNI
//! thread actually has. Running them on the main thread's 8 MiB would make a
//! stack overflow that a phone hits look fine here.

use blre_core::pack::PackStore;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let path = args.next().unwrap_or_default();

    match mode.as_str() {
        "pack" => pack(&path),
        "script" => script(&path),
        other => {
            eprintln!("mode must be pack|script, got {other:?}");
            std::process::exit(2);
        }
    }
}

fn pack(path: &str) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read: {e}");
            std::process::exit(2);
        }
    };

    let dir = std::env::temp_dir().join(format!("blre-loadall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = match PackStore::open(&dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("store: {e}");
            std::process::exit(3);
        }
    };

    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match store.import(&bytes, Some(name)) {
        Ok(out) => {
            println!(
                "ok        textures={:<5} version={:<12} name={}",
                out.pack.textures, out.pack.version, out.pack.name
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
        Err(e) => {
            println!("refused   {e}");
            let _ = std::fs::remove_dir_all(&dir);
            std::process::exit(3);
        }
    }
}

fn script(path: &str) {
    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read: {e}");
            std::process::exit(2);
        }
    };
    let source = match String::from_utf8(raw) {
        Ok(s) => s,
        Err(_) => {
            println!("refused   not UTF-8");
            std::process::exit(5);
        }
    };

    let bytes = source.len();
    // The same entry point the bridge uses: its own thread, its own stack.
    match blre_core::script_engine::run_detached(source, blre_core::script::Limits::default()) {
        Ok(report) => {
            println!(
                "ok        bytes={bytes} ran={} failed={} halted={}",
                report.ran, report.failed, report.halted
            );
            for m in report.messages.iter().take(3) {
                println!("          msg: {}", one_line(m));
            }
        }
        Err(e) => {
            println!("refused   {e}");
            std::process::exit(3);
        }
    }
}

fn one_line(s: &str) -> String {
    let mut out: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.len() > 160 {
        out.truncate(160);
        out.push('…');
    }
    out
}
