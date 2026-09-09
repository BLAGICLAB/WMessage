# AUDIT-ERROR-2026-09-08:CommandError 增加 DomainRule 变体

> 状态:worktree `todo-p0-6a` 中,未 commit / push。等用户「commit 吧」再合 main。

## 1. 动机与 bug

### Bug 1:技术债 — 19 个 TODO(P0-6A) 复制粘贴
`src-tauri/src/` 下 7 个文件散布 19 个 `// TODO(P0-6A): 无 1:1 CommandError 变体,暂走 Internal;待新增专用变体后迁移`,每次新增错误代码就复制同一 TODO 注释。代码噪声 + 维护成本,所有 error.rs 的 `match` 都不完整。

### Bug 2:更严重 — `is_recoverable: false` 错标
这 19 个错误**全部是用户可恢复/可重试的**:
- 技能暂停 → 等用户确认后继续
- URL 内网被拒 → 换 URL 重试
- 迁移进行中 → 等当前迁移完重试
- CSV 缺列 → 补列重试
- Python 未检测到 → 安装后重试
- etc.

但都被 `CommandError::Internal` 标 `is_recoverable: false`,前端拿不到「重试」按钮——**用户该能重试的反而 UI 不让重试**。这是真实的 UX bug,不只是技术债。

## 2. 新变体规格

```rust
// src-tauri/src/error.rs
DomainRule { domain: String, reason: String },
```

| 字段 | 值 |
|---|---|
| `code()` | `"DOMAIN_RULE"` |
| `is_recoverable()` | `true`(从 Internal 的 `false` 修对) |
| `message()` | `format!("[{domain}] {reason}")` |
| 序列化 | 标准 4 字段 `code/message/recoverable/platform`,前端可读 |

### 19 个 site 映射表

| Domain | 文件:行 | reason |
|---|---|---|
| `argument` | `bot.rs:1851` | taskId 与 title 冲突 |
| `argument` | `bot.rs:1875` | 缺 taskId 或 title 参数 |
| `task` | `bot.rs:1860` | taskId 未找到 |
| `task` | `bot.rs:1869` | 标题未找到 |
| `skill` | `bot_skills/runtime.rs:137` | 技能暂停 |
| `skill` | `bot_skills/runtime.rs:148` | 步数熔断 |
| `skill` | `bot_skills/runtime.rs:158` | 超时熔断 |
| `skill` | `bot_skills/state.rs:172` | 技能名无效 |
| `skill` | `bot_skills/state.rs:189` | 技能不存在 |
| `platform` | `lib.rs:50` | 复制文件不支持当前平台 |
| `clipboard` | `lib.rs:130,143,156,167` | Win 剪贴板 4 步失败 |
| `python` | `bot_py.rs:801,852,865` 等 | Python 未检测 / 应用退出 / setup 失败 / spawn 失败 |
| `migration` | `migration.rs:409,633,1160,1183` | CAS 锁 / 符号链接 / CSV 缺列 / 非法 action |
| `migration` | `migration.rs:1209` | CSV 空规则 |
| `search` | `bot_web.rs:343,371` | Bing / 百度无结果 |
| `search` | `bot_web.rs:117(merge)` | 搜索全部失败 |
| `web` | `bot_web.rs:704,708,721,727,735` | URL 缺主机 / 内网被拒 / DNS v4v6 内网 / 无公网 |
| `web` | `bot_web.rs:794,824` | 页面过大 / 无可提文本 |
| `web` | `bot_web.rs:336,366,701,777,780,792,805` | HTTP 错 / 协议错 / 重定向 / Location / 编码 |
| `csv` | `migration.rs:1183` | 非法 action |

## 3. 务实路线 vs strict cascade(关键决策)

**选择务实路线**:
- DomainRule 变体在 **tauri command 路径(直传前端)完整保留**
- tool→model 边界用 `From<CommandError> for String` impl **有损降级**:`.to_string()` 走 Display,变体代码丢失,只剩 message
- model 反正只读 string,变体给 model 是浪费

**对比 strict cascade**:
- 改所有 caller 返回类型,30+ 个 edit,cargo check 多轮
- 中间层(model dispatch)也用 CommandError,但最终又 format! 给 model
- 投资回报比:务实 = 1/3 strict 工作量,**效果在前端路径上等价**

**保留 strict 路线的能力**:
- worktree 在,有问题可回滚
- `From<CommandError> for String` 删掉,所有 `.into()` 编译报错,提示要改回 cascade

## 4. Cascade 改动清单

### 4.1 函数签名变化(`Result<_, String>` → `Result<_, CommandError>`)

| 文件 | 函数 | 备注 |
|---|---|---|
| `error.rs` | `impl From<CommandError> for String` | 新增,有损转换 |
| `lib.rs` | `copy_file_windows` | 4 个 clipboard TODO |
| `bot_skills/runtime.rs` | `step_check` | 1 个 skill TODO |
| `bot_skills/state.rs` | `load_skill_meta` | 1 个 skill TODO |
| `bot_skills/runtime.rs` | `skill_on_step` | caller 返回类型 |
| `bot_py.rs` | `run_python_ungated` | 4 个 python TODO |
| `bot_py.rs` | `run_python` | EXITING check |
| `bot_py.rs` | `py_extract_document` 等 | run_python caller(用 turbofish + map_err) |
| `bot_web.rs` | `search_bing` | 1 个 search TODO |
| `bot_web.rs` | `search_baidu` | 1 个 search TODO |
| `bot_web.rs` | `check_public_url` | 5 个 web TODO |
| `bot_web.rs` | `fetch_text` | 2 个 web TODO + 中间 format! → DomainRule |
| `bot_web.rs` | `web_search` | errs: `Vec<CommandError>` + caller return type |
| `migration.rs` | `MigrationGuard::acquire` | 1 个 migration TODO |
| `migration.rs` | `copy_dir_recursive` | 1 个 migration TODO |
| `migration.rs` | `parse_rules_csv` | 2 个 migration TODO |

### 4.2 Tool 边界降级(显式 `.into()`)

`bot.rs` 的 `tool_*_task` 函数返回 `(String, Vec<TaskRef>)` 给 model,8 个 `Err(e) => return (e.into(), Vec::new())` 显式转 String。这是务实路线的妥协:tool 边界变体信息丢失,只保 message。

## 5. 前端影响

零代码改动,但行为变化:

| 错误 | 前(Internal) | 后(DomainRule) |
|---|---|---|
| `code` | `"INTERNAL"` | `"DOMAIN_RULE"` |
| `recoverable` | `false`(错) | `true`(修对) |
| `message` | `"内部错误:xxx"` | `"[domain] xxx"` |
| 前端 switch | 不命中任何 case(死代码) | 可按 `code` 或 `domain` switch |
| 重试按钮 | 不显示(错) | 显示(修对) |

**待验证**:
- 前端有没有 `case "INTERNAL"` 的死代码?(若 switch 不命中,行为需手动确认)
- 重试按钮是否真的在 9 种场景下都出现?

## 6. 验证

### 已跑(commit 前复审,2026-09-09)
- `cargo build`:0 错(4 个 warning 全存量:files.rs 多余括号 / migration.rs unused import + dead_code / bot_fs.rs unused mut,与本次改动无关)
- `cargo test` 全量全绿:lib 577(含 5 条 DomainRule 专项)/ llm_integration 38 / memory_regression 17 / mock_llm 10 / skill_e2e 13
- 复审修复:`tool_link_file_to_task` 里重复插入的 `resolve_task` 调用已删(首份绑定从未使用,且白名单校验前多查一次 DB);`resolve_task` 末尾残留的过时 TODO(P0-6A) 注释已清;error.rs 补回文件末尾换行
- DomainRule 专项单测已补(error.rs 5 条:code 稳定 / recoverable 恒 true / message `[domain]` 格式 / 序列化 4 字段 / 同 code 不同 reason 稳定)

### 未跑
- `cargo clippy`:没跑
- 前端:vitest 应不受影响(`error.rs` 是 Rust 端,前端没动),但没跑确认
- 前端手动验证:9 种 DomainRule 场景 UI 行为未验;已确认 `src/lib/errorHandler.ts:81` 存在 `case "INTERNAL"`(对其他 Internal 错误仍是活代码,不能删,只是 DOMAIN_RULE 不再落这里)

## 7. Worktree 与合并

- **worktree 路径**:`/Users/renshi/projects/wmessage-todo-p0-6a`
- **分支**:`todo-p0-6a`
- **HEAD**:基于 wmessage main `460dc89` 前
- **main 未触碰**:`/Users/renshi/projects/wmessage` 在 main,无修改
- **回滚**:`git -C /Users/renshi/projects/wmessage worktree remove --force ../wmessage-todo-p0-6a`
- **合并**:等用户「commit 吧」(本 worktree 内)+ 切回 main + 「merge 吧」或「rebase 吧」

## 8. 后续跟进

- [x] 补 DomainRule 单元测试(error.rs 5 条,已跑绿)
- [x] 跑 `cargo test`(全绿);`cargo clippy` 仍未跑
- [ ] 前端手动验证 9 种 DomainRule 场景的 UI 行为
- [x] 检前端 `case "INTERNAL"`:`src/lib/errorHandler.ts:81` 存在,但对其他 Internal 错误仍是活代码,保留不删
- [ ] SPEC.md 加一句 error system 概述(目前只有 line 112 一行相关)
- [ ] ARCH-REFACTOR-PLAN.md 不动(它是 bot_skills 拆分 + source type 标记,跟 error system 无关)
