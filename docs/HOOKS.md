# Hook points: what injection has to reach, and how that is decided

A ModPE call does not have a function of the same name inside the game. This
document records which function each API actually needs, how that was
established, and what is still missing before anything is patched at runtime.

## The measurement

Source: `我的世界0.13怀旧_0.14.3.apk`, sha256
`245E38D6…862C13`, 19.4 MB.

```
entries        1585
lib/           lib/armeabi-v7a/libminecraftpe.so   7 711 868 bytes
               lib/armeabi-v7a/libgnustl_shared.so    661 132
               lib/armeabi-v7a/libfmod.so           1 186 316
ABIs           armeabi-v7a only
dynamic syms   30 992 (28 029 functions, 2 960 objects, 8 933 with a size)
```

Reproduce with:

```sh
cd core
cargo run --example hookplan -- /path/to/that.apk
```

## The points

Nine points, each with the ModPE surface it implements. `required` decides
whether the launcher claims injection at all; the rest degrade one API.

| id | ModPE API | symbol in this build | required |
|---|---|---|---|
| `blocksource.set` | `Level.setTile`, `Level.setBlock` | `BlockSource::setBlockAndData(int,int,int,BlockID,uchar,int)` | yes |
| `blocksource.get` | `Level.getTile`, `Level.getBlock` | `BlockSource::getBlock(BlockPos const&)` | yes |
| `level.explode` | `Level.explode` | `Level::explode(BlockSource&, Entity*, Vec3 const&, float, bool)` | yes |
| `gamemode.attack` | `Player.attack`, attack callback | `GameMode::attack(Player&, Entity&)` | yes |
| `gamemode.destroy` | `blockEventCallback`, destroy | `GameMode::destroyBlock(Player&, BlockPos, signed char)` | yes |
| `level.set_time` | `Level.setTime` | `Level::setTime(int)` | no |
| `level.get_time` | `Level.getTime` | `Level::getTime() const` | no |
| `renderer.fov` | FOV getter used by camera scripts | `LevelRenderer::getFov(float, bool)` | no |
| `block.registry` | `Block.getAllBlockIds` | `Block::mBlocks` (data, 4 bytes) | no |

## The trap this design exists to avoid

There is **no** `Level::setTile` in this library. The only two symbols containing
`setTile`/`getTile` are:

```
_ZN15SpriteComponent8setTiledEb     SpriteComponent::setTiled(bool)
_ZNK15SpriteComponent8getTiledEv    SpriteComponent::getTiled() const
```

A substring match — the obvious implementation — resolves `Level.getTile` to a
sprite renderer's boolean flag. It would not crash. It would return nonsense.
So resolution is exact-match against an explicit candidate list, and a new game
family adds a candidate rather than loosening the comparison.

`core/src/hook.rs` has a test that feeds in exactly those two lookalike symbols
and asserts both `blocksource.get` and `blocksource.set` stay missing.

## What is still not done

This step decides *whether* injection can work. It does not patch anything.

- **No runtime patching.** Replacing a function needs the game's process, its
  linker namespace, and `mprotect` on the code page. That lives in the Android
  module, not here.
- **No plts.** The `value` in a resolved point is an offset inside the `.so`, not
  a runtime address. For these functions it is the function body directly (they
  are defined and exported by the game), so an inline hook is the mechanism a
  runtime would use; the offset is where it would start.
- **32-bit ARM only.** This build is `armeabi-v7a`. A 64-bit build of the same
  era was not available, and AArch64 patches look nothing like Thumb-2 ones, so
  the arm64 path is deliberately unimplemented rather than guessed.
- **One version family.** 0.13/0.14.3. Newer builds rename things; the candidate
  list is where that shows up, and `hooks.plan` is how it gets noticed before a
  user launches a game and sees nothing work.

## Why resolve before launching

The alternative is discovering a missing symbol inside the game process, where
the failure modes are a `dlopen` abort, a SIGSEGV in a thread with no handler, or
the worst case: a hook placed at a resolved-but-wrong address. Each of those
costs a launch and produces a log that does not name the cause. Resolving from
the APK turns all of them into one sentence with a symbol in it.
