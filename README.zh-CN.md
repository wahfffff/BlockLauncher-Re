# BlockLauncher-Re

[English](README.md) | **简体中文**

面向 Android 的 **Minecraft: Pocket Edition 旧版本**启动器，核心用 Rust 从零实现，
界面用 Kotlin + Jetpack Compose Material 3。

> 与 Mojang、Microsoft 及 BlockLauncher 作者均无关联。本项目不使用、也不分发
> BlockLauncher 的任何代码、资源或素材。

---

## 下载

[Releases](https://github.com/wahfffff/BlockLauncher-Re/releases) 里有
`BlockLauncher-Re-<版本>-debug.apk` 和 `SHA256SUMS`。不需要登录，也没有外层 zip——
附件本身就是 APK。

debug 构建有两个前提：

* 它用 debug key 签名，安装前要给「打开它的那个应用」开「安装未知应用」。
* `v0.1.0` 是在签名密钥固定之前构建的，所以把之后的版本直接装到它上面会报
  「应用未安装」。卸载一次即可，之后的构建共用同一个 key，可以覆盖升级。

### 打不开怎么办

这个仓库是公开的，所以 404 基本发生在浏览器到 GitHub 这段路上，而不是仓库本身。
按这个顺序排查：

* `github.com/torvalds/linux` —— 如果它也 404 或超时，说明那个网络到不了
  GitHub，改这里没有任何用，换条网络才行。
* 链接本身。项目改过名：`wahfffff/blocklauncher-modern` 现在是跳转页，
  `wahfffff9178/…` 是另一个账号。
* `nslookup github.com`。返回的地址不在 GitHub 的段里（`20.205.*`、
  `140.82.*`）就是 DNS 污染，换 `223.5.5.5` 或挂梯子。

APK 也可以直接发文件——
[最新 release](https://github.com/wahfffff/BlockLauncher-Re/releases/latest)
不需要账号就能下载。

## 当前状态

分里程碑推进。下面这张表是**实际进度**，不是把路线图当成已完成来写。

| 部分 | 状态 | 验证方式 |
|---|---|---|
| APK 解析（二进制 `AndroidManifest.xml`、ABI 列表） | **已完成** | `cargo test` + 一个真实的 30 MB APK |
| 版本库（按内容哈希导入、去重、删除） | **已完成** | `cargo test` |
| 材质包（导入、真实贴图计数、启用/停用） | **已完成** | `cargo test` |
| ModPE 脚本运行时（全局函数、钩子、执行预算） | **已完成（未接游戏）** | `cargo test`；游戏那一侧还没有实现 |
| 原生 mod（`.nmod`）清单校验 | **已完成** | `cargo test` |
| 旧版 `.mod` 补丁（PTP 容器：作者行、载荷大小） | **只读** | `cargo test` 覆盖 8 个真实文件；载荷不会被应用，见 `docs/LEGACY-MOD.md` |
| 启动计划（ABI 选择、mod 过滤与排序） | **已完成** | `cargo test` |
| JNI 桥 | **已完成** | 桥本身由 `cargo test` 覆盖；符号名由打包检查兜底 |
| Android 界面（Compose Material 3） | **三个列表** | CI 构建并检查 APK；还没人在真机上打开过 |
| 真正的启动 / 注入 | 未开始 | — |

`cargo test` 在主机上跑 105 个用例。格式解析器的测试文件**逐字节构造**归档，
所以测试不会因为和解析器犯了同一个错误而通过。

### 现在这个 APK 能做什么

三个列表，各自有自己的存储：导入的游戏版本、导入的材质包、以及一个脚本检查器。

- **版本** —— 导入你自己已有的游戏 APK，看到包名、版本号、ABI 和 SHA-256，
  然后删掉它。**它还不会启动游戏**，启动按钮随 M3 一起来。
- **材质包** —— 导入 `.zip`，看到里面实际有多少张贴图，可以启用/停用、删除。
  一个带了清单却没有贴图的包会在导入时被拒绝——那还是用户能补救的时候。
- **脚本** —— 选一个 ModPE `.js`，交给真的运行时加载。报告会说它有没有解析成功、
  顶层代码做了什么；它调用的每个游戏函数都会如实返回「没有连接游戏」，
  而不是假装成功。带 `while (true)` 的脚本会被执行预算中断，而不是把进程挂死。

## 这个启动器不会做的事

**它不下载 Minecraft。** 游戏本体是 Mojang 的版权作品，一个托管或代下游戏包的
启动器就是在分发侵权复制品。这里没有版本下载服务，没有内置的 URL 目录，也不打
算加。

取而代之的是：**你自己导入已有的 APK。** 应用读一个本地文件，把这条版本记进
本地版本库，版本管理就这么多。手上没有 APK，启动器帮不了你。

材质包和存档同理：只导入，不代下。

## 结构

```
core/                     Rust，不依赖 Android
  src/axml.rs             二进制 XML 读取器（编译后的 AndroidManifest.xml）
  src/apk.rs              最小 ZIP 读取器 + APK 解析
  src/version.rs          Minecraft 版本比较（不是 semver）
  src/catalog.rs          磁盘上的版本库
  src/nmod.rs             原生 mod 清单与校验
  src/launch.rs           纯规划：版本 + 设备 + mod -> 启动计划
  src/bridge.rs           一条 JSON 请求进，一条 JSON 回复出
  src/jni_glue.rs         唯一的 JNI 导出；非 Android 平台为空
  examples/inspect.rs     `cargo run --example inspect -- file.apk`
  tests/core.rs           24 个主机侧用例（另有 9 个在 src/bridge.rs）
android/                  Kotlin + Compose Material 3
  app/src/main/.../NativeBridge.kt   loadLibrary + 唯一那个 external 函数
  app/src/main/.../Core.kt           类型化封装，所有调用都不在主线程
  app/src/main/.../MainActivity.kt   版本库页面
```

这样拆分的唯一目的是：**不需要手机就能测试**。外壳做的所有判断——这个版本能不能
在这台机器上跑、哪些 mod 生效、按什么顺序加载——都是 `core/` 里的纯函数。Android
那层只负责执行得到的 `LaunchPlan`，自己不做判断。

界面和核心只在一个函数上相遇：一条 JSON 命令进，一条 JSON 回复出。所以导入路径、
去重逻辑和错误码是由 `cargo test` 覆盖的，而不是靠在手机上点——一个 JNI 入口，
就只有一个地方会写错 FFI，而不是每个操作各有一个。

### 版本比较不是字符串比较

Minecraft PE 的版本号是 `0.1.1`、`0.14.0`、`1.1.5.1`、`1.16.201.01`、`0.15.0b1`
这种形态。按文本排序会把 `0.9.0` 排在 `0.14.0` 后面。所有比较都走 `GameVersion`：
按数字段解析，`b1` 这类后缀视为预发布版。

### 原生 mod

原生 mod 是被加载进游戏进程的 `.so`。对已注入的库而言沙箱是不可能的，所以唯一的
防线是**在安装阶段拒绝歧义**：

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

`entry` 不能是路径，`abis` 必须是已知 ABI，且每个声明的 ABI 必须真的提供对应的库
文件——一个声称支持 `arm64-v8a` 却只有 32 位代码的 mod，会在导入时被拒绝，而不是
在 `dlopen` 时抛出一条没人看得懂的错误。

## 构建

### 核心（任意主机）

```sh
cd core
cargo test
cargo run --example inspect -- /path/to/game.apk
```

不需要 C 编译器：四个依赖都是纯 Rust。

### Android

每次推送都会构建一个 debug APK，作为 `BlockLauncher-Re-debug` 附件挂在对应的
workflow run 上。构建过程还会**检查产物本身**：缺 `lib/arm64-v8a/libblre_core.so`、
缺 `lib/armeabi-v7a/libblre_core.so` 或缺 `classes.dex` 都会让构建失败——这三种情况
都会产出一个能装上、然后什么都不做的 APK。

手动构建需要 JDK 17、Android SDK 34、NDK r27 和 `cargo-ndk`：

```sh
cargo install cargo-ndk

cd core
cargo ndk --platform 23 \
    -t arm64-v8a -t armeabi-v7a \
    -o ../android/app/src/main/jniLibs \
    build --release

cd ../android
gradle assembleDebug        # Gradle 8.9；有意不提交 wrapper
```

`cargo ndk` 必须在 `core/` 里跑：它先调 `cargo metadata`，再看 `--manifest-path`，
所以在仓库根目录会直接报 "could not find Cargo.toml"，一行代码都没编。

APK 是 debug 版、用 debug key 签名。安装前需要给用来传输它的应用开「安装未知应用」。

## 路线图

| 里程碑 | 内容 | 通过标准 |
|---|---|---|
| M1 | 核心：APK 解析、版本库、原生 mod 校验、启动规划 | `cargo test` —— **已完成** |
| M2 | Compose Material 3 外壳 + JNI 桥 | CI 产出可安装的 APK —— **已完成**；界面还没在真机上看过 |
| M2.5 | 材质包与 ModPE 脚本运行时 | `cargo test` —— **已完成（未接游戏）**；游戏那一侧目前只是一个没有实现的 trait |
| M3 | 在独立数据目录里启动已导入的版本 | 真机报告 |
| M4 | 把脚本运行时接到跑着的游戏上，再做原生 mod 注入 | 真机报告 |

M3 和 M4 无法在构建机上验证，需要真机和真实游戏包。在那之前，它们在这份文档里
是「未验证」，而不是「已完成」。M2 的界面同理：它能编译、APK 也被检查过，
但还没有人看到它画出一屏。

M2.5 有一个属于它自己的、必须说清楚的缺口：运行时是完整的、测过的，但
`script::Host`——把 `Level.setTile` 变成对真实世界的一次调用的那个 trait——还没有
任何实现，因为没有已接入的游戏可以去实现它。这类调用会如实报告这一点，
而不是假装成功。

## 许可

MIT，见 [LICENSE](LICENSE)。
