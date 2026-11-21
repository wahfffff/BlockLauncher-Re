# BlockLauncher-Re

**English** | [简体中文](README.zh-CN.md)

A launcher for legacy **Minecraft: Pocket Edition** builds on Android, written
from scratch in Rust (core) and Kotlin + Jetpack Compose Material 3 (UI).

> Not affiliated with, endorsed by, or connected to Mojang, Microsoft, or the
> author of BlockLauncher. No code, assets or resources from BlockLauncher are
> used or redistributed here.

---

## Status

This is being built in milestones, and the table below is the truth about it
rather than the roadmap restated as if it were done.

| Part | State | How it is verified |
|---|---|---|
| APK inspection (binary `AndroidManifest.xml`, ABI list) | **done** | `cargo test`, plus a real 30 MB APK |
| Version library (content-addressed import, dedupe, remove) | **done** | `cargo test` |
| Native mod (`.nmod`) manifest validation | **done** | `cargo test` |
| Launch planning (ABI choice, mod filtering, ordering) | **done** | `cargo test` |
| Android UI (Compose Material 3) | not started | — |
| JNI bridge | not started | — |
| Actual game launch / injection | not started | — |

`cargo test` runs 24 cases on a host machine. The format readers are tested
against archives this repository *builds byte by byte* in the test file, so the
tests cannot pass by repeating the reader's own mistake.

## What this launcher will not do

**It does not download Minecraft.** Game builds are copyrighted by Mojang, and a
launcher that hosts or fetches them is distributing infringing copies. There is
no version-download service here, no bundled catalogue of URLs, and no plan to
add one.

Instead: **you import an APK you already have.** Point the app at a file, it
records the build in a local library, and that is the entire version story. If
you do not have the APK, the launcher cannot help you get one.

The same applies to textures and worlds: import, do not fetch.

## Architecture

```
core/                     Rust, no Android dependency
  src/axml.rs             binary XML reader (compiled AndroidManifest.xml)
  src/apk.rs              minimal ZIP reader + APK inspection
  src/version.rs          Minecraft version ordering (not semver)
  src/catalog.rs          the on-disk version library
  src/nmod.rs             native mod manifests and validation
  src/launch.rs           pure planning: build + device + mods -> plan
  examples/inspect.rs     `cargo run --example inspect -- file.apk`
  tests/core.rs           24 host-side cases
```

The split exists so the decisions can be tested without a phone. Everything the
shell does — deciding whether a build is runnable, which mods apply, in what
order — is a pure function in `core/`. The Android module executes the resulting
`LaunchPlan` and does nothing clever of its own.

### Version ordering is not string ordering

Minecraft PE versions are `0.1.1`, `0.14.0`, `1.1.5.1`, `1.16.201.01`,
`0.15.0b1`. Sorting them as text puts `0.9.0` after `0.14.0`. Every comparison
goes through `GameVersion`, which parses numeric segments and treats a suffix
like `b1` as a pre-release.

### Native mods

A native mod is a `.so` loaded into the game process. There is no sandboxing
possible for an injected library, so the only defence is refusing ambiguity at
install time:

```json
{
  "schema": 1,
  "id": "example-mod",
  "name": "Example Mod",
  "version": "1.0.0",
  "entry": "libexample.so",
  "abis": ["armeabi-v7a", "arm64-v8a"],
  "min_game": "0.14.0",
  "max_game": "1.1.5"
}
```

`entry` cannot be a path, `abis` must name known ABIs, and every declared ABI has
to actually ship its library — a mod that claims `arm64-v8a` and contains only
32-bit code is rejected at import instead of failing at `dlopen` with a message
nobody can act on.

## Building

### Core (any host)

```sh
cd core
cargo test
cargo run --example inspect -- /path/to/game.apk
```

No C compiler needed: the four dependencies are pure Rust.

### Android

Not buildable yet — the Compose shell is the next milestone. CI is set up to run
the core tests on every push.

## Roadmap

| Milestone | Content | Gate |
|---|---|---|
| M1 | Core: APK inspection, library, native-mod validation, planning | `cargo test` — **done** |
| M2 | Compose Material 3 shell + JNI bridge | CI produces an installable APK |
| M3 | Launching an imported build in an isolated data directory | Real-device report |
| M4 | Injecting native mods into the game process | Real-device report |

M3 and M4 cannot be verified on a build machine; they need a device and a real
game build, and until that happens they are described here as unverified rather
than announced as working.

## Licence

MIT. See [LICENSE](LICENSE).
