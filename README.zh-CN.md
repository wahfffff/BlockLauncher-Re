# BlockLauncher-Re

[English](README.md) | **简体中文**

面向 Android 的 **Minecraft: Pocket Edition 旧版本**启动器，核心用 Rust 从零实现，
界面用 Kotlin + Jetpack Compose Material 3。

> 与 Mojang、Microsoft 及 BlockLauncher 作者均无关联。本项目不使用、也不分发
> BlockLauncher 的任何代码、资源或素材。

---

## 当前状态

分里程碑推进。下面这张表是**实际进度**，不是把路线图当成已完成来写。

| 部分 | 状态 | 验证方式 |
|---|---|---|
| APK 解析（二进制 `AndroidManifest.xml`、ABI 列表） | **已完成** | `cargo test` + 一个真实的 30 MB APK |
| 版本库（按内容哈希导入、去重、删除） | **已完成** | `cargo test` |
| 原生 mod（`.nmod`）清单校验 | **已完成** | `cargo test` |
| 启动计划（ABI 选择、mod 过滤与排序） | **已完成** | `cargo test` |
| Android 界面（Compose Material 3） | 未开始 | — |
| JNI 桥 | 未开始 | — |
| 真正的启动 / 注入 | 未开始 | — |

`cargo test` 在主机上跑 24 个用例。格式解析器的测试文件**逐字节构造**归档，
所以测试不会因为和解析器犯了同一个错误而通过。

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
  examples/inspect.rs     `cargo run --example inspect -- file.apk`
  tests/core.rs           24 个主机侧用例
```

这样拆分的唯一目的是：**不需要手机就能测试**。外壳做的所有判断——这个版本能不能
在这台机器上跑、哪些 mod 生效、按什么顺序加载——都是 `core/` 里的纯函数。Android
那层只负责执行得到的 `LaunchPlan`，自己不做判断。

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

暂时无法构建——Compose 外壳是下一个里程碑。CI 已经在每次推送时跑核心测试。

## 路线图

| 里程碑 | 内容 | 通过标准 |
|---|---|---|
| M1 | 核心：APK 解析、版本库、原生 mod 校验、启动规划 | `cargo test` —— **已完成** |
| M2 | Compose Material 3 外壳 + JNI 桥 | CI 产出可安装的 APK |
| M3 | 在独立数据目录里启动已导入的版本 | 真机报告 |
| M4 | 把原生 mod 注入游戏进程 | 真机报告 |

M3 和 M4 无法在构建机上验证，需要真机和真实游戏包。在那之前，它们在这份文档里
是「未验证」，而不是「已完成」。

## 许可

MIT，见 [LICENSE](LICENSE)。
