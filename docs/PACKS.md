# Resource packs: what gets accepted, and why

A pack is a `.zip` the game reads from its own data directory. This launcher
imports it, counts what it carries, and keeps it; it does not claim the game
accepted it. The only authority on that is the game.

The interesting part is the question "is this a resource pack at all", because
the answer was wrong before and the wrongness was invisible: the error said the
archive was empty when it was a working pack.

## The rule

An archive is a pack when it carries **something the game loads** — an image
(`.png`, `.tga`, `.jpg`) or a shader (`.fragment`, `.vertex`, `.glsl`) — and is
**not an APK**.

That is deliberately about content rather than about directory names. Directory
names differ per pack, and a pack zipped from a folder arrives wrapped in a
directory whose name is whatever the author called it.

The APK check comes first, by the two entries every APK has
(`AndroidManifest.xml` + `classes.dex`). An APK contains images too, so counting
images alone would accept a 40 MB game as a texture pack and install it, where
the game silently ignores it.

## The measurement

Seven real packs from the 0.14 era, run through `core::pack::PackStore::import`
(one process each, via `core/examples/loadall.rs`):

| archive | top level | before | now |
|---|---|---|---|
| `FaithfulPE 64x For 0.14.2&3 V1.zip` | `assets/`, `pack.mcmeta`, `pack.png` | accepted, 302 | accepted, 270 images |
| `faithful PE 32x 修复版V1.zip` | `assets/` | accepted, 164 | accepted, 140 images |
| `Paper Cut-Out[折纸]16x 0.14.x V1 .zip` | `images/`, `pack.mcmeta` | **refused** | accepted, 318 images |
| `Gun mod（配套材质）.zip` | `Ultimacraft_Texture/armor/…` | **refused** | accepted, 279 images |
| `我的世界CF手游主题材质包分享.zip` | `cf主题0.4-wzj/assets/images/…` | **refused** | accepted, 188 images |
| `苦力怕光影V8.zip` | `Creeper'S V8.0 0.14.x/assets/…` | **refused** | accepted, 4 images |
| `冷色光影0.14.3.zip` | `renderchunk.fragment`, `renderchunk.vertex` | **refused** | accepted, 2 shaders |

Two accepted, five refused. The count column is why the counts look smaller now:
the old rule counted *every entry under `assets/`*, including `sounds/`,
`.lang` files and JSON, and reported that as a texture count.

## Why the old rule was wrong

It was a whitelist of directory prefixes — `textures/`, `assets/`,
`texture_packs/` — matched against the entry path. Every one of the five failures
is a layout that whitelist does not describe:

* `images/` at the root instead of `assets/images/` (this is the MCPE 0.11–0.14
  layout; the `assets/` level came later).
* A wrapper directory: the author zipped the folder rather than its contents.
* A shader pack: two files, no directories, no images at all.

None of these is malformed. They are the format's own variety, and a rule that
calls them empty is a rule about our convenience rather than about the packs.

## Kinds

`Pack.kind` is `texture` when the archive has images and `shader` when it only has
shaders. The UI shows the kind, because a card that always said "0 张贴图" would
describe a working shader pack as an empty one.

`Pack.shaders` counts shader files. Both fields have `serde(default)` so an index
written before they existed still loads — the upgrade path is a user who already
has packs.

## Metadata

Two files are read, both optional:

* `manifest.json` — modern-ish. `header.name`, `header.version`,
  `header.description`.
* `pack.mcmeta` — what 0.14-era packs actually carry.
  `pack.description` only; it has no name field worth trusting.

Both are searched at the archive root **and** under the wrapper directory, since a
zipped folder puts them one level down. A missing or unreadable manifest is not
fatal: the pack still has assets the game can use, and the name falls back to the
filename.

## Known limits

* Nothing here proves the game will use the pack. It proves the archive contains
  what a pack contains.
* A pack whose images are inside a *second* archive (a zip inside the zip) is not
  unpacked. None of the seven does this.
* The `.mod` files that ship alongside these packs are a different format
  (`0xFF 'P' 'T' 'P'` + a name + a Thumb code blob), not a resource pack, and are
  not handled here.
