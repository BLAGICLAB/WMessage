# WMessage Tauri Commands 危险等级矩阵

> **目的**：盘点全部 60 个 `#[tauri::command]`，给出危险等级 + 是否过 audit / middleware 闸门，
> 作为后续安全审计的入口。
>
> **生成方式**：基于 `grep -rE "#\[tauri::command\]" src-tauri/src/` + `lib.rs:invoke_handler`
> 全清单，按函数名 + 参数 + 文件位置推断；具体实现风险待 Sprint H 二轮精读 `middleware.rs` +
> `tool_guard.rs` 复核。
>
> **基线**：`e1dd21e`（2026-09-14）

## 风险等级图例

- 🔴 **HIGH** — 网络外发 / 进程启停 / 文件删除 / 凭据读写 / 任意代码执行
- 🟠 **MED** — 文件读写 / 持久化配置 / DB 写
- 🟡 **LOW** — 只读 / 状态查询 / 弹窗
- ⚪ **INFO** — 无副作用的常量导出

## 审计覆盖标签

- **AUDIT** — 命令体内有 `audit_xxx()` 调用（结构化审计日志）
- **MW** — 命令是否走 middleware 闸门（pre_execute / pre_step）
- 标注 `?` 表示本次未深读、待 Sprint H 复核

---

## 全部 60 命令

| # | 模块::命令 | 危险 | 操作 | AUDIT | MW | 备注 |
|---|---|---|---|---|---|---|
| 1 | `lib::copy_file_with_title` | 🟡 | 文件读 + 剪贴板写 | ? | ? | 平台 helper 三平台 cfg |
| 2 | `lib::focus_main_window` | ⚪ | 窗口聚焦 | ? | ? | 无副作用 |
| 3 | `bot_skills::open_file_path` | 🟡 | 启动 OS 打开文件 | ? | ? | tauri-plugin-opener |
| 4 | `bot_skills::pick_files_dialog` | ⚪ | 弹文件选择器 | ? | ? | tauri-plugin-dialog |
| 5 | `bot_skills::delete_bound_file` | 🔴 | **物理 / 软删文件** | ? | ? | 经 `trash` crate，走回收站 |
| 6 | `db::bind_files` | 🟡 | 文件路径解析 + DB 写绑定 | ? | ? | 仅 path resolve + is_dir |
| 7 | `db::db_load` | ⚪ | 任务读 | ? | ? | 全部任务一次性读 |
| 8 | `db::db_upsert` | 🟠 | **DB 写（任务增删改）** | ? | ? | 行级 upsert |
| 9 | `db::db_delete` | 🔴 | **DB 删（任务）** | ? | ? | 软删 + bound file 处理 |
| 10 | `db::tasks_export` | 🟠 | **文件写（任务导出）** | ? | ? | 用户指定路径 |
| 11 | `db::tasks_import` | 🟠 | **文件读 + DB 写** | ? | ? | 按 id 并集合并 |
| 12 | `db::workspace_load` | ⚪ | 工作区读 | ? | ? |  |
| 13 | `db::workspace_upsert` | 🟠 | **DB 写（工作区）** | ? | ? |  |
| 14 | `db::workspace_delete` | 🔴 | **DB 删（工作区）** | ? | ? |  |
| 15 | `db::workspace_export` | 🟠 | **文件写（工作区导出）** | ? | ? |  |
| 16 | `db::workspace_import` | 🟠 | **文件读 + DB 写** | ? | ? |  |
| 17 | `db::bot_history_load` | ⚪ | 机器人历史读 | ? | ? | 按 session 拉取 |
| 18 | `db::bot_history_save` | 🟠 | **DB 写（机器人历史）** | ? | ? |  |
| 19 | `db::bot_history_clear` | 🟠 | **DB 写（清空历史）** | ? | ? |  |
| 20 | `db::bot_sessions_load` | ⚪ | 机器人会话列表读 | ? | ? |  |
| 21 | `db::bot_session_create` | 🟠 | **DB 写（创建会话）** | ? | ? |  |
| 22 | `db::bot_session_delete` | 🔴 | **DB 删（会话）** | ? | ? | 级联删 history |
| 23 | `db::bot_session_rename` | 🟠 | **DB 写（重命名）** | ? | ? |  |
| 24 | `api_handlers::api_start` | 🟠 | 启动本地 HTTP server | ✅ | ? | loopback only，tiny_http vendor patch |
| 25 | `api_handlers::api_stop` | 🟠 | 停止本地 HTTP server | ✅ | ? |  |
| 26 | `api_handlers::api_status` | ⚪ | HTTP server 状态 | ✅ | ? |  |
| 27 | `api_handlers::api_rotate_token` | 🟠 | **API token 轮换** | ✅ | ? | token 持久化到 api_enabled.flag |
| 28 | `bot_slash::bot_get_enabled` | ⚪ | 机器人开关读 | ? | ? |  |
| 29 | `bot_slash::bot_set_enabled` | 🟠 | 机器人开关写 | ? | ? | 持久化 |
| 30 | `bot::bot_get_config` | ⚪ | 机器人配置读 | ? | ? |  |
| 31 | `bot::bot_set_config` | 🟠 | **机器人配置写（含 API key 写入 keyring）** | ? | ? | 走 keyring |
| 32 | `bot::bot_clear_api_key` | 🔴 | **keyring 删（清空 API key）** | ? | ? | keyring delete |
| 33 | `memory::consolidate::memory_consolidate_now` | 🟠 | **记忆整理（DB 写 + LLM 调用）** | ? | ? | 走 ORT + 可能 LLM |
| 34 | `bot_chat::bot_chat` | 🔴 | **网络外发（LLM 流式）+ 工具调用** | ✅ | ✅ | 主聊天入口 |
| 35 | `bot_chat::bot_execute_task` | 🔴 | **网络外发 + 执行任务** | ✅ | ✅ | 含 Python/dotnet 子进程 |
| 36 | `bot_slash::bot_stop` | 🟠 | 停止机器人执行 | ? | ? | 设 stop flag |
| 37 | `bot_chat::bot_compact` | 🟠 | **网络外发（LLM）** | ✅ | ? | 历史压缩 |
| 38 | `bot_slash::bot_confirm_response` | 🟡 | 确认响应回传 | ? | ? | oneshot channel |
| 39 | `bot_artifacts::confirm_artifact_batch` | 🟡 | 产物批次确认 | ? | ? |  |
| 40 | `bot::bot_log_read` | ⚪ | bot.log 读 | ? | ? |  |
| 41 | `bot_py::py_get_enabled` | ⚪ | Python 开关读 | ? | ? |  |
| 42 | `bot_py::py_set_enabled` | 🟠 | Python 开关写 | ? | ? | 持久化 |
| 43 | `bot_py::py_env_check` | 🟡 | Python/dotnet 环境探测 | ? | ? | 调 `python --version` |
| 44 | `bot_skills::skills_list` | ⚪ | skill 列表读 | ? | ? |  |
| 45 | `bot_skills::skills_import` | 🟠 | **文件读（skill zip）** | ? | ? | 含解压 + 写入 skills_dir |
| 46 | `bot_skills::skills_delete` | 🔴 | **skill 文件 / 目录删除** | ? | ? | 走 `trash` |
| 47 | `bot_skills::skills_open_dir` | ⚪ | 打开 skills_dir | ? | ? | tauri-plugin-opener |
| 48 | `profile::profile_get` | ⚪ | profile 读 | ? | ? |  |
| 49 | `profile::profile_set_name` | 🟠 | **profile.json 写** | ? | ? | 含原子 rename |
| 50 | `profile::profile_set_avatar` | 🟠 | **头像文件写 + profile.json 写** | ? | ? | 校验 mime/size |
| 51 | `profile::profile_remove_avatar` | 🔴 | **头像文件删除 + profile.json 写** | ? | ? |  |
| 52 | `migration::migration_rules_load` | ⚪ | CSV 规则读 | ? | ? |  |
| 53 | `migration::migration_rules_import` | 🟠 | **CSV 文件读** | ? | ? | 含校验 |
| 54 | `migration::migration_rules_template_save` | 🟠 | **CSV 模板写** | ? | ? |  |
| 55 | `migration::migration_log_read` | ⚪ | migration.log 读 | ? | ? |  |
| 56 | `migration::migration_run` | 🔴 | **文件移动 / 删除（按规则）** | ? | ? | 走 `trash` + 文件 mv |
| 57 | `migration::migration_status` | ⚪ | 迁移状态读 | ? | ? |  |
| 58 | `consts::app_consts` | ⚪ | 常量导出 | ⚪ | ⚪ | 纯常量 |
| 59 | `error::*` (1) | ⚪ | 错误码常量 | ⚪ | ⚪ | 纯 enum |
| 60 | `bot::*` (1) — `bot_log_read` | ⚪ | 见 #40 | ? | ? | 与 #40 重复计数校正 |

> 校正：`#60 = #40 重复`。实际 invoke_handler 注册 58 个；其他 2 个 `[tauri::command]` 在 `bot.rs` 顶部 + `consts.rs` 中作为非 invoke_handler 标记（用于元数据 / 测试）。**总活跃命令数 = 58**。

## 高危命令汇总（🔴 共 9 个）

| 命令 | 主要操作 |
|---|---|
| `bot_skills::delete_bound_file` | 文件/目录软删 |
| `db::db_delete` | 任务删除（级联文件处理）|
| `db::workspace_delete` | 工作区删除 |
| `db::bot_session_delete` | 会话删除（级联 history）|
| `bot::bot_clear_api_key` | keyring API key 删除 |
| `bot_chat::bot_chat` | LLM 流式 + 工具链 |
| `bot_chat::bot_execute_task` | LLM + 任务执行（Python/dotnet 子进程）|
| `bot_skills::skills_delete` | skill 文件/目录删除 |
| `profile::profile_remove_avatar` | 头像文件删除 |
| `migration::migration_run` | 批量文件移动/删除 |

**Sprint H 必须精读**：每个 🔴 命令的 audit / middleware 路径是否覆盖完整，特别是错误路径。

## 中危命令（🟠 共 13 个）

文件读写 + DB 写 + 配置写。Sprint H 抽查 3-5 个即可。

## 低危 + 只读（🟡 + ⚪ 共 36 个）

风险较低，但仍有边界问题（如 `pick_files_dialog` 跨平台路径格式、`focus_main_window` 多窗口焦点）。

---

## 路径保留 grep（不变量 1 自检）

```bash
# 前：60 个 #[tauri::command] 全清单
rg -c "crate::<原模块>::" src-tauri/src/ > /tmp/before.txt
# 后：Sprint C/D/E 切片完成后，再 grep
rg -c "crate::<原模块>::" src-tauri/src/ > /tmp/after.txt
diff /tmp/before.txt /tmp/after.txt   # 必须为空
```

---

## TODO（待 Sprint H）

- [ ] 精读 `middleware.rs` + `tool_guard.rs`，把所有 🔴 的 AUDIT / MW 列从 `?` 改为 ✅/❌
- [ ] 抽查 🟠 的 audit 调用，列遗漏项
- [ ] 把 `app_consts` / `error::*` 等纯常量从「60 命令」中拆出，本表只列真正 invoke_handler 注册的 58 个
- [ ] 与 `tool_guard::tests::dead_command_bind_file_not_in_dispatcher` 等回归锁对账，确认 dead command 已被锁定不再注册

## 修订记录

- 2026-09-14：Sprint A3 首次产出（基于 `e1dd21e` HEAD 的 invoke_handler 清单）