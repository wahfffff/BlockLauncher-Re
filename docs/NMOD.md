# nmod: the native mod package

This is the format ModdedMCPE / EnderCore use, and the one BlockLauncher's
"native mods" became once the game grew an add-on system of its own. It is not
invented here: `core/src/nmod.rs` is a port of the reference's reading,
validation and override code, and the places it is deliberately different are
listed at the bottom.

Reference (read, not copied): `InPie/ModdedMCPE-Android`, mostly
`endercore/src/main/java/org/endercore/android/mod/nmod/` (`NModPackage`,
`NMod`), `.../operator/instance/NModPreparer.java` and `.../overrider/*`.

## Package layout

An nmod is **a zip with `nmod_manifest.json` at its root**. Everything else is
addressed from there:

```
nmod_manifest.json                          required, at the root
icon.png, banner.png                        referenced by the manifest
android/<support>/assets/<path>             overrides for the game's assets/
android/<support>/<abi>/lib<name>.so        libraries loaded into the game
```

`<support>` is the `name` of a `game_supports` entry. The reference declares
`NMod.NMOD_PLATFORM = "android"`; other platforms are parsed and skipped, not
rejected, because a package is allowed to ship more than one.

## Manifest

| Field | Required | Notes |
|---|---|---|
| `uuid` | yes | must parse as a UUID; it is the package's identity across updates |
| `name` | yes | shown in the list |
| `version_name` | yes | |
| `version_code` | yes | integer; the reference uses it to refuse a downgrade |
| `manifest_version` | yes | |
| `min_sdk_version` | yes | refused when greater than the runtime's `SDK_INT` |
| `target_sdk_version` | yes | |
| `game_supports` | yes | array, at least one useful entry |
| `author` | no | warning when absent |
| `author_email` | no | warning when absent |
| `description`, `change_log` | no | carried, not displayed yet |
| `icon`, `banner` | no | warning when absent; **error** when named but not in the zip |

The reference models "absent" as `-1` for integers and `null` for strings and
rejects those explicitly. This port keeps the same required/optional split, but
reports the missing field by name (`manifest has no min_sdk_version`) instead of
the reference's sentence, because the person fixing it is the one who packed it.

### `game_supports[]`

| Field | Required | Meaning |
|---|---|---|
| `platform` | yes | `"android"` for anything this build acts on |
| `name` | yes | one directory name under `android/` |
| `target_game_versions` | yes | **regular expressions**, matched against the game's version *name* |
| `native_libs[]` | no | `{ "name": "...", "main": "..." }`; `main` is carried, unused |
| `file_overrides[]` | no | `{ "path": "..." }` |
| `json_overrides[]` | no | `{ "path": "...", "mode": "replace" \| "merge" }` |
| `text_overrides[]` | no | `{ "path": "...", "mode": "replace" \| "append" \| "prepend" }` |

**`target_game_versions` are patterns, not version numbers.** The reference
matches with `Pattern.matches(version, gameVersionName)`, which is a *full*
match. So `0\.14\..*` covers the 0.14 family, `.*` covers everything, and
`0.14` does not cover `0.14.3`. This port compiles every pattern at import time
with `regex-lite`, full-matched, so a typo is reported before the game is
launched instead of at launch.

Selection walks `game_supports` in manifest order and takes the first support
with any matching pattern — the same first-match rule as the reference.

## Overrides

Each override names a file *inside the mod's own `assets/`*, and is applied onto
the game's extracted `assets/` tree:

| Kind | Read from | Written to | Modes |
|---|---|---|---|
| `file_overrides` | named file | `assets/<path>` | copy |
| `json_overrides` | named file | `assets/<path>` | `replace`, `merge` |
| `text_overrides` | named file | `assets/<path>` | `replace`, `append`, `prepend` |

Order is the reference's: **file, then json, then text**. It only matters when
two entries name the same path, but that combination is real (copy a file, then
append a line), and swapping the order changes the result.

`append` and `prepend` need the destination to exist — they are edits, and
silently creating the file would turn a broken pack into a plausible one.

### `merge` is asymmetric, on purpose

The reference's `JSONMerger` merges the **game's** document into the **mod's**:

* keys only the mod has are added,
* keys both have: the **game's** value wins,
* arrays: the mod's elements stay, the game's are appended after them,
* a key that is an object on one side and an array/scalar on the other is an
  error rather than a guess.

That looks upside down next to `file_overrides` (where the mod wins), but it is
what the reference does, and a "port" that merged the other way would silently
change the output of every existing pack. It is asserted in two tests.

## Native libraries

The reference patches the class loader's native library directory to
`android/<support>/<abi>/` and then calls `System.loadLibrary(name)` for each
entry, so `"name": "mymod"` resolves `libmymod.so`.

This core does the part that can be checked without a device: given a support
and an ABI it reports the exact zip entries to load, accepting either
`lib<name>.so` or the literal name (some packs ship the prefix already).
Deciding *which* ABI is the device's is `launch::build`'s job.

## What is implemented, and how it is verified

| Step | State | Verified by |
|---|---|---|
| Read the zip and the root manifest | done | `cargo test` (deflated and stored fixtures) |
| Required/optional field validation, warnings | done | one test per rule |
| Reject a manifest that is not at the root | done | test |
| Reject an override with no payload in the zip | done | test names the path it looked for |
| Reject a bad override mode / bad version pattern | done | tests |
| Skip non-`android` platforms | done | test |
| First-match support selection, full-match regex | done | test (`10.14.3` does not match `0\.14\..*`) |
| Apply file / json / text overrides | done | end-to-end test on a temp assets tree |
| JSON merge semantics | done | three tests, including the array and type-mismatch cases |
| Native library discovery per ABI | done | test |
| Launch planning over a set of packages | done | `tests/core.rs` |

**Not implemented** (and therefore not claimed): extracting the game's assets
from the APK, writing the extracted tree, patching the class loader, and calling
`dlopen`. Those live in the Android module and need a device; `docs/MODPE-API.md`
and the README's roadmap say the same thing about the script host.

## Deliberate differences from the reference

Three, each because the reference's behaviour is a bug rather than a feature:

1. **Paths may not escape `assets/`.** The reference joins the manifest string
   onto the assets directory, so `"../../..."` writes outside it. This refuses
   absolute paths, `..`, backslashes and NUL, and caps the length.
2. **A bad regex is an import error.** The reference only compiles the pattern
   when the game launches, where a typo becomes "this mod does nothing".
3. **A json merge that hits mismatched types is reported, not half-applied.**
   The reference's merger throws mid-way; here the caller gets the key name.

One thing is *not* different even though it looks wrong: the merge direction in
the section above. That is the reference's behaviour and is kept.
