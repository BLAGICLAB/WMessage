# WMessage 架构改造计划 + 验收清单（2026-08-19）

> 用法：一次只做一阶段，每完成一小项就在 `[ ]` 里打 `x`。每阶段结束跑一次全量验证，
> 全绿才进下一阶段；任何阶段停下来，代码都处于可发布状态。
> Token 控制原则：阶段间独立、每阶段改动最小、不做计划外清理、复用现有测试验证。

## 验证命令（每阶段结束必跑）

```bash
cd src-tauri && cargo test 2>&1 | tail -5
cd .. && npx vitest run 2>&1 | tail -5
npx tsc --noEmit
```

---

## Phase A：写入口 source 标记类型化（最高优先，防回归）

目标：消灭 `source:"bot"/"api"/"migration"` 字符串约定，改成编译期可检查的枚举。

改动点：
- [x] A1. 新建 `src-tauri/src/mutation.rs`：`pub enum MutationOrigin { Main, Widget, Bot, Api, Migration }`，derive `Serialize/Deserialize/Clone/Copy`，实现 `as_str()`（返回现有字符串值，保证前端协议不变）
- [x] A2. `src-tauri/src/lib.rs` 加 `mod mutation;`
- [x] A3. `bot.rs:711-718` `broadcast_after_mutation` 的 `"bot"` 字面量 → `MutationOrigin::Bot`
- [x] A4. `api_handlers.rs:899-902` 的 `"api"` → `MutationOrigin::Api`
- [x] A5. `migration.rs:710-715` 的 `"migration"` → `MutationOrigin::Migration`
- [x] A6. 新建 `src/lib/mutationOrigin.ts`：`export type MutationOrigin = "main"|"widget"|"bot"|"api"|"migration"` + `isMutationOrigin()` 类型守卫
- [x] A7. `App.tsx:198-206` 事件 payload 的 source 判断改用 `isMutationOrigin()`，非法值走 WARN 日志而不是静默吞掉
- [x] A8. 全局 grep `"source"` 相关字符串字面量，确认无遗漏的裸字符串（`Grep '"bot"|"api"|"migration"'` 限定事件 emit/listen 处）
- [x] A9. 跑验证命令三件套，全绿（另修复两处预存问题：middleware 签名扩展后 11 处集成测试调用点失配、error.rs doctest 伪代码块，均已修）
- [x] A10. 手动冒烟：主窗口改任务 → 挂件同步正常；bot 改任务 → 主窗口合并不回写（2026-08-19 通过）

## Phase B：共享常量单一来源（Rust 真相，前端启动拉取）

目标：`MAX_TASK_FILES`、标题/备注上限、图片扩展名等不再前后端手写两份。

改动点：
- [x] B1. `src-tauri/src/db.rs`（或新 `consts.rs`）新增 `#[tauri::command] pub fn app_consts() -> AppConsts`，返回 `{ max_task_files, max_title, max_note, image_exts }`，值取自已有的 Rust 常量（落地为新 `consts.rs`；`bot_chat.rs::IMAGE_EXTS` 改 pub）
- [x] B2. `lib.rs` invoke_handler 注册 `app_consts`
- [x] B3. 新建 `src/lib/consts.ts`：启动时 `invoke("app_consts")` 拉一次缓存为模块级单例，拉取失败回退到现有硬编码值（保证纯前端测试环境可用）（`main.tsx` 启动调用 `loadAppConsts()`，不阻塞首屏）
- [x] B4. `src/lib/taskFiles.ts:10` 的 `MAX_TASK_FILES` 改为从 consts 读取（保留同名导出，调用方零改动）（活绑定 re-export）
- [x] B5. `ChatPanel.tsx:12` 图片扩展名清单改从 consts 读取（`imageExtSet()`，use 点读取）
- [x] B6. 前端测试：mock `app_consts` invoke，断言 fallback 与正常路径各一条（`src/lib/consts.test.ts`，含活绑定同步断言）
- [x] B7. 跑验证命令三件套，全绿（cargo 421 通过 0 失败 / vitest 105 通过 / tsc 无错）

## Phase C：拆分 bot_skills.rs（3091 行 → 子模块）

目标：纯移动代码，不改任何逻辑；`mod.rs` re-export 保持调用方零改动。

改动点：
- [x] C1. 建 `src-tauri/src/bot_skills/` 目录，`bot_skills.rs` → `bot_skills/mod.rs`（git mv 保留历史）
- [x] C2. 拆 `parse.rs`：SKILL.md frontmatter + DSL 步骤解析（592 行，含对应测试）
- [x] C3. 拆 `state.rs`：`SkillRun` 状态机 + `SKILL_RUNS` 全局表 + 终态清理（256 行，含 #[cfg(test)] 辅助）
- [x] C4. 拆 `scheduler.rs`：`run_skill_scheduler` auto 模式调度器（789 行，含 DslOutcome/DslFailure/persist_outcome_quiet/skill_terminate_all）
- [x] C5. `mod.rs` 只留 `pub use` re-export + 模块头注释；`lib.rs`/`bot.rs`/`bot_chat.rs` 的 import 不变（另按职责拆出 files.rs/manage.rs/vars.rs/runtime.rs，全部 ≤1200 行；仅 7 处可见性放宽到 pub(crate)，逻辑零改动）
- [x] C6. `cargo test` 全绿（本阶段不碰前端）（421 通过 0 失败，与拆分前基线一致；`cargo check --release` 亦通过）
- [ ] C7. 手动冒烟：bot 执行一个 skill，auto 模式和 ask 模式各一次

---

## 总验收（三阶段全做完后）

- [ ] V1. 三件套全绿（cargo test / vitest / tsc）
- [ ] V2. `grep -rn '"source": *"' src-tauri/src src/` 无裸字符串字面量（除 mutationOrigin 定义处）
- [ ] V3. `MAX_TASK_FILES` 全局只剩 Rust 一处定义
- [ ] V4. `bot_skills.rs` 单文件消失，目录下各子文件 ≤ 1200 行
- [ ] V5. 手动全流程：绑文件/文件夹、bot 聊天、bot 执行任务卡、挂件同步、归档规则各过一遍
- [ ] V6. DEVLOG.md 补一条本次改造记录
