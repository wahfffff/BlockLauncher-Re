# BlockLauncher-Re

**English** | [简体中文](README.zh-CN.md)

A launcher for legacy **Minecraft: Pocket Edition** builds on Android, written
from scratch in Rust (core) and Kotlin + Jetpack Compose Material 3 (UI).

> Not affiliated with, endorsed by, or connected to Mojang, Microsoft, or the
> author of BlockLauncher. No code, assets or resources from BlockLauncher are
> used or redistributed here.

---

## Download

[Releases](https://github.com/wahfffff/BlockLauncher-Re/releases) hold
`BlockLauncher-Re-<version>-debug.apk` and a `SHA256SUMS` file. No login and no
zip wrapper — the APK is the asset.

Two caveats that come with a debug build:

* It is signed with a debug key, so installing it needs "install unknown apps"
  enabled for whatever app you use to open it.
* `v0.1.0` was built before the signing key was pinned, so installing a later
  build over it fails with "app not installed". Uninstall it once; builds after
  that share one key and update in place.

### If the page does not open

This repository is public, so a 404 is almost always somewhere between the
browser and GitHub rather than in the repository. In this order:

* `github.com/torvalds/linux` — if that also 404s or times out, GitHub is not
  reachable from that network and no change here can help. A different
  connection can.
* The URL. The project was renamed: `wahfffff/blocklauncher-modern` is a
  redirect page now, and `wahfffff9178/…` is a different account.
* `nslookup github.com`. An answer outside GitHub's ranges (`20.205.*`,
  `140.82.*`) means DNS poisoning; `223.5.5.5` or a VPN fixes it.

The APK can also just be sent as a file — it sits on the
[latest release](https://github.com/wahfffff/BlockLauncher-Re/releases/latest),
which needs no account to download.

## Status

This is being built in milestones, and the table below is the truth about it
rather than the roadmap restated as if it were done.

| Part | State | How it is verified |
|---|---|---|
| APK inspection (binary `AndroidManifest.xml`, ABI list) | **done** | `cargo test`, plus a real 30 MB APK |
| Version library (content-addressed import, dedupe, remove) | **done** | `cargo test` |
| Texture packs (import, real texture count, enable/disable) | **done** | `cargo test` |
| ModPE script runtime (globals, hooks, bounded execution) | **done off-device** | `cargo test`; the game side is not attached yet |
| Native mod (`.nmod`) manifest validation | **done** | `cargo test` |
| Legacy `.mod` patches (PTP container: author line, payload size) | **read only** | `cargo test` against 8 real files; the payload is not applied — see `docs/LEGACY-MOD.md` |
| Launch planning (ABI choice, mod filtering, ordering) | **done** | `cargo test` |
| JNI bridge | **done** | `cargo test` for the bridge; the symbol name is checked by the APK build |
| Android UI (Compose Material 3) | **three lists** | CI builds and inspects the APK; nobody has looked at it on a phone yet |
| Actual game launch / injection | not started | — |

`cargo test` runs 105 cases on a host machine. The format readers are tested
against archives this repository *builds byte by byte* in the test file, so the
tests cannot pass by repeating the reader's own mistake.

### What the APK does today

Three lists, each backed by its own store: imported game builds, imported texture
packs, and a script checker.

- **Builds** — import a game APK you already have, see its package, version, ABIs
  and SHA-256, delete it. It does not start the game yet; an imported build is a
  file in a list, and the launch button arrives with M3.
- **Texture packs** — import a `.zip`, see how many images (or shaders) it
  actually carries, enable or disable it, delete it. Shape is not policed: a
  wrapper directory, an `images/` root, `assets/` and a shader-only archive all
  import, because all four are layouts real 0.14 packs ship. An APK is refused by
  name, which is the one archive that would otherwise be installed silently and
  do nothing. See [docs/PACKS.md](docs/PACKS.md) for the layouts and the
  measurement behind them.
- **Scripts** — pick a ModPE `.js` and have the real runtime load it. The report
  says whether it parsed, what its top-level code did, and every game call it
  made comes back as "no game is attached" rather than silently succeeding. A
  script with `while (true)` is stopped by an execution budget instead of hanging
  the process.

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
  src/pack.rs             texture packs: import, texture count, enable/disable
  src/script.rs           the ModPE contract: Host trait, API table, Limits
  src/script_engine.rs    those globals bound to a JavaScript engine (Boa)
  src/nmod.rs             native mod manifests and validation
  src/launch.rs           pure planning: build + device + mods -> plan
  src/bridge.rs           one JSON request in, one JSON reply out
  src/jni_glue.rs         the single JNI export; empty off-device
  examples/inspect.rs     `cargo run --example inspect -- file.apk`
  tests/core.rs           24 host-side cases (+46 in src/)
android/                  Kotlin + Compose Material 3
  app/src/main/.../NativeBridge.kt   loadLibrary + the one external function
  app/src/main/.../Core.kt           typed wrapper, every call off the main thread
  app/src/main/.../MainActivity.kt   three lists: builds, packs, script check
```

The split exists so the decisions can be tested without a phone. Everything the
shell does — deciding whether a build is runnable, which mods apply, in what
order — is a pure function in `core/`. The Android module executes the resulting
`LaunchPlan` and does nothing clever of its own.

The UI and the core meet at exactly one function: a JSON command string in, a
JSON reply out. That is why the import path, the deduplication and the error
codes are covered by `cargo test` rather than by tapping on a phone — one JNI
entry point is one place for an FFI mistake instead of one per operation.

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

Every push builds a debug APK and uploads it as the `BlockLauncher-Re-debug`
artifact on the workflow run. The build also *inspects* the result: it fails if
`lib/arm64-v8a/libblre_core.so`, `lib/armeabi-v7a/libblre_core.so` or
`classes.dex` is missing, because each of those produces an APK that installs
cleanly and then does nothing.

To build by hand you need JDK 17, Android SDK 34, NDK r27 and `cargo-ndk`:

```sh
cargo install cargo-ndk

cd core
cargo ndk --platform 23 \
    -t arm64-v8a -t armeabi-v7a \
    -o ../android/app/src/main/jniLibs \
    build --release

cd ../android
gradle assembleDebug        # Gradle 8.9; no wrapper is committed on purpose
```

`cargo ndk` has to run from `core/`: it shells out to `cargo metadata` before it
looks at any `--manifest-path`, so from the repository root it stops with
"could not find Cargo.toml" before compiling anything.

The APK is a debug build, signed with the debug key. Installing it needs
"install unknown apps" enabled for whatever you copy it with.

## Roadmap

| Milestone | Content | Gate |
|---|---|---|
| M1 | Core: APK inspection, library, native-mod validation, planning | `cargo test` — **done** |
| M2 | Compose Material 3 shell + JNI bridge | CI produces an installable APK — **done**; on-device look still unreviewed |
| M2.5 | Texture packs and the ModPE script runtime | `cargo test` — **done off-device**; the game side is a trait with no implementation yet |
| M3 | Launching an imported build in an isolated data directory | Real-device report |
| M4 | Attaching the script runtime to the running game, then injecting native mods | Real-device report |

M3 and M4 cannot be verified on a build machine; they need a device and a real
game build, and until that happens they are described here as unverified rather
than announced as working. The same caution applies to the M2 UI: it compiles and
the APK is inspected, but no one has yet seen it draw a screen.

M2.5 has one honest caveat of its own: the runtime is complete and tested, but
`script::Host` — the trait that turns `Level.setTile` into a call on a live
world — has no implementation, because there is no attached game to implement it
against. Every such call reports that instead of pretending.

## Licence

MIT. See [LICENSE](LICENSE).
