# The ModPE surface: where it comes from and what runs

## Provenance

The JavaScript API a BlockLauncher script calls is a list of names and calling
conventions. This repository did not guess them, and it did not copy any code:
`tools/modpe_api.py` reads the DEX of a BlockLauncher APK the user already owns
and prints **class names, method names, parameter types, return types and access
flags only**. It does not read method bodies, does not emit bytecode, does not
extract string constants and never opens a resource.

Reading the APK gives an authoritative answer to "what did `Level.getTile`
actually take", which is exactly the question a compatibility layer has to
answer. Reimplementing behaviour from the signatures is the work; copying the
implementation is not attempted and the tool cannot do it.

```
$ python3 tools/modpe_api.py "BlockLauncher Pro.apk"
39 classes, 636 public methods

net.zhuoweizhang.mcpelauncher.ScriptManager              (282)
net.zhuoweizhang.mcpelauncher.ScriptManager$NativeEntityApi   (68)
net.zhuoweizhang.mcpelauncher.ScriptManager$NativePlayerApi   (55)
net.zhuoweizhang.mcpelauncher.ScriptManager$NativeLevelApi    (53)
net.zhuoweizhang.mcpelauncher.ScriptManager$NativeModPEApi    (30)
net.zhuoweizhang.mcpelauncher.ScriptManager$BlockHostObject   (29)
... 33 more
```

The namespaces map to the globals a script sees:

| DEX class | Script global | Public methods |
|---|---|---|
| `ScriptManager$NativeLevelApi` | `Level` | 53 |
| `ScriptManager$NativePlayerApi` | `Player` | 55 |
| `ScriptManager$NativeEntityApi` | `Entity` | 68 |
| `ScriptManager$NativeModPEApi` | `ModPE` | 30 |
| `ScriptManager$BlockHostObject` | `Block` | 29 |

`ScriptManager` itself hosts the callbacks (`newLevel`, `modTick`, `chatHook`,
`selectLevelHook`, …) — 282 public methods, most of which are hook plumbing
rather than script-facing API.

## What this build runs today

The runtime is Boa, pure Rust. `core/src/script.rs` declares the contract
(`Host`), the table (`API`), and the hook names; `core/src/script_engine.rs`
binds the table to the engine. Both are covered by tests that execute real
JavaScript.

Implemented: `print`, `clientMessage`, `Level.getTile`, `Level.setTile`,
`Level.getMinecraftVersion`, `Level.explode` (throws, see below),
`Player.getX/getY/getZ/setPosition`, `Entity.getAll`, `ModPE.readData`,
`ModPE.saveData`, `ModPE.removeData`. Hooks: `newLevel`, `modTick`,
`chatHook`, `selectLevelHook`.

Not implemented: the remaining ~620 methods. They are not stubbed to return
zero — a call that reaches a global with no game behind it throws, naming itself:

```
Level.explode(0, 1, 2, 4) is not wired to a game yet
```

A script that appears to run and silently changes nothing is harder to debug
than one that stops at the first unsupported call and says which one.

## The seam, and why there is one

Every global is an adapter over `Host`. `NullHost` refuses everything with one
sentence; the Android module will supply a `Host` that talks to the running
game. That is what makes the semantics testable off-device: `script_tests.rs`
drives the real engine against a recording host and asserts what a script sees
and what it changes.

It also means the script names are decided once. Adding `Level.getBiome` is a
row in `API`, a match arm in `install`, and a test — not a new special case.

## Known gaps

* **No instruction budget.** A `while (true)` blocks the calling thread. Scripts
  therefore run on their own thread, not the UI thread, but there is no deadline
  and a spinning script is killed with the process. This is the next safety item.
* **No game binding yet.** Everything above is verified against a fake world.
  Which `Host` methods the real game can answer is a device question.
* **No script store in the UI yet.** Import/list is the next commit; the runtime
  does not depend on it.
