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

## Execution budget

A script is untrusted input, so it runs under `script::Limits` rather than until
it decides to stop:

| Ceiling | Default | What it catches |
|---|---|---|
| `loop_iterations` | 500 000 per frame | `while (true)`, `do … while`, `for (;;)` |
| `recursion` | 256 nested calls | a script that recurses instead of looping |
| `stack_size` | 10 240 value slots | absurdly deep expressions |

The unit is a count, not a wall-clock deadline. A deadline measured in
milliseconds behaves differently on a fast phone and a slow one, so a script
could pass on one and fail on another, and the tests would be flaky for a reason
that has nothing to do with the code. A count is identical everywhere and can be
asserted exactly.

When a ceiling is passed the engine throws, and the call returns a `Report` with
`halted: true` and a message naming the budget — not a hang, and not a bare
"script error" that hides which ceiling was hit.

The defaults come from a measurement: this engine runs about 1.3 million
iterations of a trivial loop per second in a release build, so 500 000 is a few
hundred milliseconds of work. No honest mod script comes close; a runaway one is
cut off well before it matters.

## Known gaps

* **No game binding yet.** Everything above is verified against a fake world.
  `script::Host` has no implementation, so on a device every game call returns
  "no game is attached". Which `Host` methods the real game can answer is a
  device question, not a build-machine one.
* **A single non-returning engine call is still uninterruptible.** The budget
  counts loop iterations and calls; nothing can interrupt the engine inside one
  operation. That is why scripts run on their own thread rather than the UI
  thread.
* **No persisted script store.** The UI checks a script the user picks; it does
  not keep a list of installed scripts the way builds and texture packs are kept.
  The runtime does not depend on that.

## Where a script runs

Scripts do not run on the calling thread. script_engine::run_detached starts a
thread with 16 MiB of stack and runs the engine there, because a JNI thread is
given 1 MiB and the engine's parser walks a left-leaning tree recursively: a long
`a + a + a …` chain or a long `else if` chain needs more stack than that, and
exhausting a stack cannot be caught — the process aborts, so the app disappears
with no error to read.

Measured on the real scripts that exposed it: four of the five in the sample
payload overflow 1 MiB and all four complete at 2 MiB. 16 MiB is an eightfold
margin over that threshold, and costs virtual address space rather than memory.

Two shapes that are *not* the problem, also measured: bracket nesting (max 21
across those scripts) and plain statement count (8000 flat `var` statements
parse in 1 MiB).
