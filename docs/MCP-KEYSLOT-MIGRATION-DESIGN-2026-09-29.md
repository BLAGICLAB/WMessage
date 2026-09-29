# MCP env/headers 迁移 KeySlot — 设计方案（待拍板，未动代码）

> 日期：2026-09-29 · 拍板背景：AUDIT-FIX-PLAN-2026-09-29 §9-③「MCP env/headers 明文存 bot-config.json → 迁 KeySlot；先出方案，拍板后再动代码」。
> 本文回答拍板要求的三件事：**现有明文 key 的迁移路径 / 迁移失败的回滚 / 涉及文件（配置加载、UI 保存、日志脱敏）**。

## 0. 结论先行（推荐方案）

**每台服务器一个机密 blob，存系统凭据存储；bot-config.json 永不再落 env/headers 明文。** 完整复用仓库既有先例——Tavily/Brave key 的「`#[serde(skip_serializing)]` 读旧迁新」模式（types.rs:95-104 + io.rs `migrate_search_keys`），不新造机制。

三行版本：
1. `McpServerConfig.env/.headers` 加 `#[serde(skip_serializing)]`（可反序列化=认老配置和前端 payload，永不序列化=**任何忘剥 key 的写路径 fail-closed**，与 tavily_key 同款注释语义）；
2. 新增 `bot/mcp/secrets.rs`：按服务器 id 存取 blob（keyring 同 service 加 `mcp:<id>` 条目；Linux 无 dbus 降级单文件 0600）；`load_config` 读时水合（带进程内缓存），保存/删除命令落/清 blob；
3. 迁移在 `CONFIG_WRITE_LOCK` 内、**任何配置写之前**执行（老明文 → keyring → 读回校验 → 备份原文件 → 写回剥离），失败保留明文原样、WARN 审计、下次重试。

## 1. 范围

**做**：stdio `env` 全部值 + http `headers` 全部值（含非敏感项——见 §6 待拍板①）。
**不做**：args/command/url 等非敏感字段（继续明文，它们是启动语义的一部分）；主 LLM/Tavily/Brave key（已在 keyring，不动）；MCP 配置的导出/导入功能（当前无此功能，若未来加，导出**不含**机密是默认语义，需明示）。

## 2. 存储设计

| 项 | 设计 | 依据 |
|---|---|---|
| keyring service | 复用 `KEYRING_SERVICE`（同 service = 复用应用钥匙串 ACL，macOS 不新增授权弹窗） | keyring.rs 现状 |
| keyring user | `mcp:<server_id>`（id 是 uuid simple，字符集安全；每服务器一条，上限 20 台） | MAX_MCP_SERVERS=20 |
| blob 内容 | `{"env":{...},"headers":{...}}` JSON（BTreeMap 序列化，键序稳定） | 与配置字段同构 |
| Linux 降级 | 单文件 `bot-mcp-secrets.json`（数据目录，0600，`{ "<id>": blob }`，tmp+rename 原子写），WARN 审计 `mcp.secret_fallback_plaintext`（每进程一次） | 对齐 bot-api-key.txt 策略 + api_auth.rs 原子写惯例 |
| 体积上限 | 单服务器 blob 序列化 ≤ 2048 字节，保存时校验，超限响亮报错 | **Windows 凭据 blob 上限 2560 字节**（CRED_MAX_CREDENTIAL_BLOB_SIZE）；现有 env 值单条上限 4096 字节，超限组合存不进 Windows 凭据管理器，必须前置拦截 |
| 进程内缓存 | `Mutex<HashMap<id, McpSecrets>>`：首次水合填充，save/delete 时更新；`mcp_status` 等高频读路径不再反复敲 keychain | mcp_status 是同步命令、设置页轮询调用 |

## 3. 涉及文件与改动点（拍板问③）

| 文件 | 改动 |
|---|---|
| `bot/mcp/config.rs`（配置层） | `env`/`headers` 加 `#[serde(skip_serializing)]` + 注释对齐 tavily_key 语义；新增纯函数内核：`secrets_blob(&server) -> McpSecrets`、`apply_secrets(&mut server, blob)`、`has_inline_secrets(&server) -> bool`（迁移判据）；模块头「敏感边界」注释改写（明文声明作废，指向 secrets.rs） |
| `bot/mcp/secrets.rs`（**新文件**） | blob 结构（serde）；`read/write/delete/has(server_id)` 按后端分发（复用 `key_backend()` 探测，**不改 keyring.rs**）；降级文件 IO（0600 + tmp+rename）；`hydrate_into(cfg)`（读路径水合，缓存命中不敲 keychain；keyring 故障 → 填空 + WARN `mcp.secret_read_failed`，**不炸配置加载**——服务器连不上时 UI 状态点可见）；`purge(server_id)` |
| `bot/config/io.rs` | `load_config` 解析后调 `secrets::hydrate_into`；`migrate_mcp_server_secrets_locked(app)`（§4）挂在 `with_locked_config` 现有 `migrate_bot_config_schema_locked` 旁；启动路径（`lib.rs` setup / 现有迁移调用点）同加一次（幂等 + once 标记） |
| `bot/mcp/commands.rs` | `mcp_server_save`：payload 里 env/headers → 写 blob（**先 keyring 后配置**，写败则整体报错不动配置）→ upsert；`mcp_server_delete`：锁内 `purge(id)`（配置删除成功才清 blob，清败 WARN 留孤儿可手动清）；`mcp_server_toggle` 不涉密（只动 enabled），水合值不会被写回（skip_serializing） |
| `bot/mcp/manager.rs` | **零改动**：reload/call 读的 cfg 来自 `load_config`（已水合） |
| 前端 `McpPanel.tsx` / `types.ts` | **几乎零改动**：编辑回填走 `bot_get_config`（水合后含机密，表单行为不变）；保存 payload 结构不变；SaveConfirmDialog 已展示 env + 打码 headers（B0-4 已做），文案加一句「机密存系统钥匙串，不再明文落配置文件」 |
| 日志/审计 | **现状已合规**：`audit()` 只记 id/name/transport/enabled 且过 `escape_for_log`（round-1 已核实无 key 落日志）；新增审计事件（migrated/migrate_failed/read_failed/fallback_plaintext）只带 id + 条数 + 错误分类，**绝不带值**。回归测试加一条：save 带机密后全量 grep bot.log 与 bot-config.json 无值 |
| `tests-audit/` / 错误码 | 不新增 tauri 命令、不改错误码 → bridge/parity 门禁无感 |

## 4. 迁移路径（拍板问①）

时机：**每次 `CONFIG_WRITE_LOCK` 内、读配置之前**（挂在与 schema 迁移同一位置）+ 启动时一次。顺序即安全性：

```
锁内：
1. 原样解析 bot-config.json（水合前——文件里的明文还是真相源）
2. 找 env/headers 非空的服务器；没有 → 置迁移完成标记，返回
3. 首台迁移前：备份 bot-config.json → bot-config.backup-mcp-keys.json（0600），
   审计 mcp.secret_backup_created（一次性；备份文件含明文——它就是回滚网，见 §5）
4. 逐台：blob → keyring 写入 → **读回比对一致** 才算成功
5. 全部成功 → 原子写回配置（skip_serializing 自动剥离明文）→ 审计 mcp.secret_migrated（id + 条数）
   任一台失败 → **不写回**，配置保持原样（明文还在），WARN mcp.secret_migrate_failed，下次重试
```

幂等性：剥离后的配置 `has_inline_secrets`=false → 迁移短路；keyring 已有条目被同内容覆盖 = 无害。**关键不变式：剥离明文的唯一途径是「keyring 已确认写入」之后的写回**——skip_serializing 使任何其他写路径都不可能把明文写回盘，迁移失败时明文仍在文件里，零丢失窗口。

## 5. 失败与回滚（拍板问②）

| 故障 | 行为 | 用户可见 |
|---|---|---|
| keyring 写失败（钥匙串锁定等） | 步骤 4 失败 → 不写回、配置原样；WARN 审计 | 无感（下次启动重试）；连续失败时设置页 MCP 面板顶部提示条（二期，可选） |
| 读回比对不一致 | 同上按失败处理 | 同上 |
| 保存时 keyring 不可用（UI 主动保存） | `mcp_server_save` 整体报错，配置不动 | 明确错误文案「机密写入系统凭据存储失败：…」，表单保留 |
| 迁移后 keyring 读取失败（日常运行） | `hydrate_into` 填空 + WARN `mcp.secret_read_failed` | 该服务器连接失败（缺鉴权），状态点红、stderr 尾巴可见——**绝不崩配置加载** |
| **整体回滚（功能级）** | 退出应用 → 把 `bot-config.backup-mcp-keys.json` 改回 `bot-config.json` → 启动。备份文件在 keyring 数据完好的情况下可删；删除前审计提示 | 文档化手动步骤（README 维护节） |
| 删除服务器时 purge 失败 | 配置删除已成功（不回滚删除），WARN + keyring 留孤儿条目 | 无感；`mcp.secret_purge_failed` 审计可追溯，重装同 id 概率≈0 |

「不把用户配置搞坏」的三重保证：剥离前必有已验证的 keyring 副本；剥离动作本身被 skip_serializing 收口到唯一写路径；永远有一份 0600 明文备份兜底。

## 6. 待拍板子项（不阻塞方向，只定细节）

1. **迁移粒度**：推荐 **A1=env/headers 全量迁**（模型简单、零判断、覆盖「非敏感项以后变敏感」）；备选 A2=UI 逐项勾「敏感」只迁勾选项（配置文件保留非敏感项可读性，代价：每项多一个开关 + 用户判断负担 + 漏勾=明文）。本文按 A1 展开。
2. **备份文件保留策略**：推荐永久保留（0600，占几 KB）+ 设置页提供「删除迁移备份」按钮（二期）；最简方案是不做按钮、文档给手动路径。
3. **blob 体积上限**：推荐 2048 字节/服务器（Windows 2560 留余量）；超限时用户需拆分 env 或缩短 token。

## 7. 测试计划（对齐 keyring.rs 可测内核注入风格）

1. 纯内核：blob serde 往返 / `has_inline_secrets` / 剥离等价性（skip_serializing 前后 diff）。
2. 降级后端注入（PlaintextFile + 临时文件）：blob 读写删、0600、原子写、损坏文件容忍（读败=空+WARN 不炸）。
3. 迁移幂等：明文配置 → 迁移 → 文件无明文 + keyring 有值；再迁移 = no-op；keyring 写失败（注入错误后端）→ 配置原样。
4. 命令集成（临时数据目录）：save 带机密 → 文件 grep 无值 + bot.log grep 无值 + blob 可读回；delete → blob 清；toggle → 机密不受影响。
5. 体积上限：>2048 字节 blob 保存响亮报错。
6. 回归锁：`bot-config.backup-mcp-keys.json` 在首迁时生成；skip_serializing 断言（对齐 `mcp_servers_roundtrip_keeps_entries_and_skips_empty_shape` 风格）。

## 8. 工作量与节奏

- secrets.rs + config.rs 字段改造 + io.rs 迁移：约 1 个批次（M）；commands.rs/前端文案：S；测试：约占该批一半。
- 建议排在 **B1 之后、B4-6 原定位置**：等 B1（自进化数据安全）落地后单开一批，避免与 B1 的 CONFIG_WRITE_LOCK 改动打架。
- 本方案获批 → 按上述落实现；若选 A2，§2/§3 增加每项敏感标记字段与 UI 开关，工作量 +S。
