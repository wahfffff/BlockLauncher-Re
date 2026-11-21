# Legacy `.mod` patches

A `.mod` from the BlockLauncher era is a 15–40 byte file in a container whose
magic is `ff 50 54 50`. It is not a library and it is not a script: it is a
patch that some other tool used to apply to `libminecraftpe.so`.

This document records exactly what the launcher reads, how that was established,
and what it deliberately does not claim.

## The container

```
offset  size  meaning
0       4     magic: ff 50 54 50
4       2     container version, big-endian — 1 in every sample
6       2     header flags — 0 in every sample
8       2     offset of the 0x00 that ends the author line
10      n     the author line, ASCII, may be empty (n = that offset - 10)
10+n    1     0x00
11+n    ...   payload
```

The offset field is not redundant with "scan for the next zero": it is what
makes the field a parsed structure rather than a byte scan, and it is asserted in
the tests — for each of the eight samples the declared offset lands exactly on
the terminating zero, and is exactly `10 + len(line)`.

## What is proven, and what is not

**Proven.** All eight samples parse, and each one's declared offset lands on its
terminator:

| file | bytes | author line |
|---|---|---|
| `9格物品栏.mod` | 18 | *(none)* |
| `友善怪物(Friendly Monster).mod` | 16 | *(none)* |
| `夜视(Night Vision).mod` | 34 | `Made by 500ISE` |
| `夜视.mod` | 34 | `Made by Skull.` |
| `服务器255人上限(255 Server Slots).mod` | 28 | `Made by KSYMC` |
| `死亡时保留背包物品(Inventory Save).mod` | 28 | `Made by KSYMC` |
| `高清修复(No Rotate Texture).mod` | 16 | *(none)* |
| `高速燃烧(Fast Flames).mod` | 15 | *(none)* |

**Not proven — the payload.** It is plausibly a search pattern plus a
replacement: the two night-vision patches both end in

```
4f f0 7e 50    mov.w r0, #0x7e
70 47          bx lr
```

which is a plausible "night vision returns true" edit. But that reading does not
survive contact with the rest of the corpus:

* four of the eight payloads are an odd number of bytes (4, 4, 4, 7 — after the
  terminator), and Thumb instructions come in pairs;
* no payload carries a length, an address, or a digest of the bytes it replaces,
  so there is nothing to locate *where* the edit goes;
* the two `SKYMC` patches and the two `item slot` patches have no trailing
  `70 47`, so "always ends with a return" is false.

Reading the payload as "the thing to write" and guessing an offset into a live
`libminecraftpe.so` is precisely how a patch corrupts a function instead of
editing one. So this build does not do it.

## What the launcher does with them

`core::mods::scan` lists a folder and classifies each entry:

* a directory containing `nmod.json` → **native**, `usable: true` when the
  manifest validates and every promised ABI has its library;
* a `.mod` file that parses → **legacy**, `usable: false`, with the reason on the
  row;
* a `.mod` file that does not parse → **legacy**, `usable: false`, with the byte
  offset it failed at.

The point is that a user who drops a `.mod` into the app gets a row saying
"legacy patch by KSYMC (4 bytes, cannot run)" instead of "unknown format". One of
those sends them looking for a bug; the other explains the situation.

## Rules this reader follows

* **Unknown header flags are refused, not ignored.** The field is zero in every
  sample, so a non-zero value means a variant whose layout is unknown — and
  reading its author-line offset as if it were the known layout produces
  plausible garbage rather than an error.
* **Every failure names the byte offset.** These files are small enough that the
  user can look at the exact byte; "invalid .mod" would not distinguish a corrupt
  download from the wrong file.
* **A file over 64 KiB is refused unread.** The payload is tens of bytes; a
  200 MB file with a `.mod` extension is not this format.
* **The extension is matched case-insensitively.** `.MOD` is the same file, and
  dropping it silently is worse than listing it.

## If you want to move this forward

The missing piece is not more parsing — it is one worked example: a `.mod`, the
`libminecraftpe.so` it was made for, and a diff of that library before and after
someone applied it. With that, the payload can be matched against the binary and
the encoding falls out. Without it, any implementation is a guess that will
corrupt a live game.
