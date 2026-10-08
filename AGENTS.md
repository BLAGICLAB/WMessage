# AGENTS.md — AI 代理工作约定

给在本仓库工作的编码代理。基线全文见 `SPEC.md`「开发基线」，本文件只放
每次干活都要的第一屏；改了门禁或约定时同步这里（本文件过长即失职）。

## 地图

- `docs/testing.md` — 门禁清单、失败修法、豁免方式
- `docs/rust-bot-architecture.md` — 模块树，现行架构以此为准
- `SPEC.md` — 现行产品规格（功能规格全景）+ 开发基线（准绳）；功能变更随提交更新它
- `DEVLOG.md` — 只写「为什么换方向」；git log 是事实记录

## 命令

- 快检：`bash scripts/test-fast.sh`（pre-commit 自动跑，按改动智能跳过）
- 全量：`bash scripts/test-all.sh`（pre-push 自动跑：cargo nextest + vitest）
- Rust 单测：`cd src-tauri && cargo test --lib`；lint：`cargo clippy --manifest-path src-tauri/Cargo.toml`
- 前端：`npm test`；lint：`npx oxlint src`；类型：`npx tsc --noEmit`

## 硬性禁令（门禁会拦，人也退）

- 注释不带工单号/日期决策/拍板记录（`W8-ATTACH`、`OCR C5`、`B0-1`、`AUDIT-*`、「拍板」）——这些只进 DEVLOG；不画横幅分隔线。注释只写代码说不出的约束。
- 不吞错误：禁空 catch、禁 `unwrap()`/`expect()` 绕错、禁 `as any` 逃逸类型——要么处理要么上抛。
- 不留死代码：本次改动孤儿化的函数/导入/依赖随同一提交删除；未要求的无关遗留不顺手重构。
- 不加投机抽象：trait/中间件/config 开关等，第二个调用方出现前不抽象（YAGNI）。
- 密钥只走系统 keyring / 环境变量，不进源码、不进仓库。

## 工作方式

- 动手前工作区是干净 commit（checkpoint）；小步提交，一个逻辑变更一个 commit。
- 行为变更换测试；修 bug 先写复现测试再修。
- 新增依赖先给一行理由：stdlib / 现有依赖为何不行。
- 提交前人眼看一遍 diff——门禁只懂规则，不懂意图。
