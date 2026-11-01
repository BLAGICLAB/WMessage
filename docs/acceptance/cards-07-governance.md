# ACC-G · 治理与记忆沉淀（L2 实机卡）

> 覆盖 `SPEC.md`「5. 治理与记忆沉淀」3 条功能（5-103 ~ 5-105），对应矩阵 `docs/acceptance/matrix.csv` 的 D5 域。
> 判据写法与证据规范见 `docs/ACCEPTANCE-PLAN-2026-10-30.md`；审计事件名取自 `docs/acceptance/event-names.md` 白名单。
> 数据目录（macOS）：`~/Library/Application Support/com.renshi.wmessage/`（便携模式随 exe 走，见 `docs/acceptance/cards-08-platform.md` ACC-P-03）。下文的 `<data_dir>` 指该目录。
> 日志检索统一写法：`grep -F "<事件名>" "<data_dir>/bot.log" | tail -20`。

### ACC-G-01 · 记忆整理与沉淀：ops 同事务直写 + 四条 lesson 渠道

- **SPEC**：5-103
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：—（无：全库无实机卡）
- **前置**：应用已启动；机器人总开关已打开；模型可用；记忆库里已有 ≥20 条记忆（含 ≥2 条 lesson）
- **步骤**：
  1. 设置页 → 「记忆」分区 → 「记忆整理」卡，看开关状态与「上次整理」时间，点「立即整理」。
  2. 记下完成提示里的三个数字（合并 / 提炼 / 解决矛盾各多少）。
  3. 回「记忆库」列表，看本轮整理后条目数与新增条目的 kind。
  4. 聊天里让机器人「记住：验收时先开日志再看界面」，等它回复；随后在记忆库列表里找这条 lesson。
  5. 再让机器人「记住：验收时先开日志再看界面」（几乎同义的一句），刷新记忆库列表看行数变化。
  6. 在「记忆库」里手动改写一条已有记忆的内容，看改后行数与 kind。
- **判据**：
  1. 【可观测】「记忆」分区有独立的「记忆整理」卡，含启用开关与「立即整理」按钮；点后按钮变「整理中…」，完成时给出提示「整理完成：合并 N 条、提炼 N 条规律、解决 N 处矛盾」（N 为整数，0 合法）。
  2. 【可审计】`<data_dir>/bot.log` 中出现 `memory.consolidate_now`（检索命令：`grep -F "memory.consolidate_now" "<data_dir>/bot.log" | tail -20`）。
  3. 【可审计】定时整理到期跑批时 `<data_dir>/bot.log` 中出现 `memory.consolidate`；LLM 不可用时出现 `memory.consolidate_failed`（该轮静默放弃，但审计里留痕）。
  4. 【可观测】整理产出的条目落在记忆库列表里且 kind 不是 lesson——**整理不产 lesson**，行为准则只由下面四条渠道沉淀。
  5. 【可观测】步骤 4：模型主动记下的教训在记忆库列表里 kind=lesson，且出现在下一轮机器人的「经验教训」上下文里（下一轮回答能引用到它）。
  6. 【可观测】步骤 5：几乎同义的两条不新增行（行数不增），那条的内容是后写的一句——cos≥0.92 合并即替换。
  7. 【可观测】步骤 6：改写一条记忆后行数不增、kind 与该行内容自洽（不会出现「内容是 lesson、kind 仍是 fact」的行）。
  8. 【可核对】`grep -n "fn record_lesson_core" -A 30 src-tauri/src/memory/mod.rs` 命中，且能看到 kind=lesson / importance=4 / source=system / tags 首元素为 lesson——四条渠道共用这一个落库口。
  9. 【可核对】`grep -rn "record_lesson_core" src-tauri/src/` 的调用点覆盖四条渠道：模型主动记（`src-tauri/src/memory/mod.rs`）、任务失败沉淀（tags 含 task_exec）、技能失败沉淀（`src-tauri/src/bot_skills/runtime.rs`，scenario 为 skill:<名>）、用户纠正聚簇（`src-tauri/src/bot/corrections.rs`）。
  10. 【可核对】`grep -n "DEDUP_MERGE_COSINE\|DEDUP_HINT_COSINE" src-tauri/src/memory/store.rs` 命中 0.92 与 0.75 两个阈值——合并与冲突提示的分界。
- **证据**：截图（「记忆整理」卡 + 完成提示 + 记忆库列表前后对比）+ `<data_dir>/bot.log` 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（`memory.consolidate` / `memory.consolidate_failed` / `memory.consolidate_now` 三个都 grep 得到才算名还在）
- **缺口补验**：「记忆整理」全库无命中：ops 同事务直写、四条 lesson 渠道、cos≥0.92 合并即替换未验

### ACC-G-02 · 文件治理：授权四档 + per-tool 权限规则表

- **SPEC**：5-104
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：M43（`docs/realmachine-mutation-test-2026-10-24.md`）、M83（`docs/realmachine-smoke-test-2026-10-25.md`）、W05（`docs/realmachine-mutation-test-2026-10-24.md`）
- **前置**：应用已启动；机器人总开关已打开；模型可用；准备一个**白名单外**的路径（例如 `/tmp/wmessage-acceptance-probe.txt`，先自己写好）
- **步骤**：
  1. 进「聊天」，看输入卡底栏 🛡 授权模式 pill 与点开后的下拉（记下四档文案）。
  2. 切到 `strict`，让机器人读上面那个白名单外文件。
  3. 切到 `ask`，让机器人读同一个文件；弹出授权窗后分别试「拒绝」与「始终允许该目录」。
  4. 切到 `auto`，让机器人读同一个文件。
  5. 切到 `yolo`，让机器人读同一个文件；观察 pill 颜色。
  6. 设置页 → 「机器人」分区 → 「工具权限规则（按工具覆盖授权模式）」→ `+ 添加规则`，加一条 `deny` 某工具（如 `read_text_file`）的规则并保存；回聊天让机器人调该工具。
  7. 再加一条 `ask` 某工具的规则并保存，重试该工具。
- **判据**：
  1. 【可观测】授权模式下拉由严到宽四档：`strict` / `ask` / `auto` / `yolo`，每档右侧给一句完整语义——ask=「白名单外弹授权窗」、strict=「白名单外直接拒绝，不弹窗」、auto=「白名单内自动放行，白名单外仍弹窗」、yolo=「全部放行不弹窗（仍记审计）」。
  2. 【可观测】`strict` 档读白名单外文件：被硬拒，**不弹授权窗**，工具返回明确的拒绝说明。
  3. 【可观测】`ask` 档读同一个文件：弹授权窗，窗内含 60s 倒计时、「拒绝 / 确认」两个出口，白名单外路径另给三选一（含「始终允许该目录」）；点「始终允许该目录」后该目录进入白名单。
  4. 【可观测】`auto` 档读同一个白名单外文件：**仍然弹授权窗**（auto 只放行白名单内）。
  5. 【可观测】`yolo` 档读同一个文件：不弹窗、直接放行；pill 同时转成警示色（设置页里给出「⚠️ 不弹任何授权」的整段说明）。
  6. 【可审计】`<data_dir>/bot.log` 中按档位出现对应事件（检索命令：`grep -F "bot_fs.denied" "<data_dir>/bot.log" | tail -20`）：strict → `bot_fs.denied`；ask 放行 → `bot_fs.ask_allow_once` 或 `bot_fs.ask_allow_always`；ask 拒绝 → `bot_fs.ask_denied`；yolo → `bot_fs.yolo_allow`。
  7. 【可审计】切档后 `<data_dir>/bot.log` 中出现 `bot.perm_mode_set`（检索命令：`grep -F "bot.perm_mode_set" "<data_dir>/bot.log" | tail -20`）。
  8. 【可审计】步骤 6/7 命中 per-tool 规则时 `<data_dir>/bot.log` 中出现 `tool_rule.hit`（检索命令：`grep -F "tool_rule.hit" "<data_dir>/bot.log" | tail -20`）；`deny` 规则命中时该工具被硬拒，**规则优先于全局授权模式**。
  9. 【可核对】`grep -n "bot_set_perm_mode" src-tauri/src/bot/config/commands.rs` 命中——写盘走窄命令只动 `perm_mode`，不整份替换写（不抹掉设置页并发改的白名单与开关）。
  10. 【可核对】`grep -n "PermMode::Strict\|PermMode::Ask\|PermMode::Auto\|PermMode::Yolo" src-tauri/src/bot/config/types.rs` 命中四档枚举；`grep -n "tool_rules" src-tauri/src/bot/config/types.rs` 命中 per-tool 规则表字段（deny/ask/allow 首中即停，无命中走全局档）。
- **证据**：截图（四档下拉 + 每档的弹窗/放行结果 + 设置页规则表）+ `<data_dir>/bot.log` 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（`bot.perm_mode_set` / `tool_rule.hit` / `bot_fs.denied` 三者任一 grep 不到 = 卡片过期；grep 得到但机器上没有对应事件 = 应用侧回归）
- **缺口补验**：授权四档与 per-tool 规则表未验；精确命中与软链逃逸已验

### ACC-G-03 · 密钥纪律：key 不落明文配置 + 厂商用自己的 key

- **SPEC**：5-105
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：M14、M15、M16、M17（`docs/realmachine-mutation-test-2026-10-23.md`）、S04、W08（`docs/realmachine-regression-test.md`）
- **前置**：应用已启动；设置页可进「模型设置」「MCP 服务」「机器人」三个分区；已有一个厂商条目可填 key
- **步骤**：
  1. 设置页 → 「模型设置」→ 某厂商条目 → 填一个**可辨认的假 key**（如 `sk-ACC-FAKE-KEY-0001`）→ 保存。
  2. 在磁盘上搜这个假 key：`grep -c "sk-ACC-FAKE-KEY-0001" "<data_dir>/bot-config.json"`。
  3. 设置页 → 「MCP 服务」→ 某台服务器 → 填 header/env 里的机密值 → 保存。
  4. 同样在磁盘上搜那个机密值；再看 MCP 配置文件里该处是不是空串。
  5. 清掉刚填的假 key（留空保存），把磁盘上的假 key 手动塞回配置文件对应字段，重启应用，看启动后配置文件与日志。
  6. 给一个**没配 key** 的厂商条目发一次请求（或触发它的探活），看界面报什么。
- **判据**：
  1. 【可观测】步骤 1 保存后：厂商条目页显示 key 已存入凭据存储（输入框不再回显明文）。
  2. 【可核对】步骤 2：`<data_dir>/bot-config.json` 里搜不到该假 key（`grep -c` 返回 0）——API key 不落明文配置。
  3. 【可核对】步骤 4：MCP 配置文件里对应字段是空串，机密值搜不到——MCP 机密同样走系统凭据存储。
  4. 【可审计】`<data_dir>/bot.log` 中出现 `config.plaintext_key_stripped`（写盘前把盘上残留旧明文安置进凭据存储并剥除时；检索命令：`grep -F "config.plaintext_key_stripped" "<data_dir>/bot.log" | tail -20`）。
  5. 【可审计】搜索 key 从老配置迁移进凭据存储时出现 `config.search_key_migrated`；MCP 侧迁移成功出现 `mcp.secret_migrated`、备份原文件出现 `mcp.secret_backup_created`。
  6. 【可审计】凭据存储不可用时 MCP 侧回落保留明文，出现 `mcp.secret_fallback_plaintext` 或 `mcp.secret_migrate_failed`；回滚路径出现 `mcp.secrets_rollback_ok` / `mcp.secrets_rollback_failed`——**安置不进凭据存储就中止本次写盘**，宁可这次配置没存上也不丢密钥。
  7. 【可观测】步骤 6：厂商条目没配 key 时，请求侧明确报缺 key；探活请求不带鉴权，且不带鉴权的 2xx **不被**转成「已验证」（该厂商仍显示未验证）。
  8. 【可观测】厂商条目用自己的 key：给某厂商填它自己的 key 后，发请求走该厂商条目（不借用全局主 key），该厂商的 base_url 不被带出去做探活。
  9. 【可核对】`grep -n "fn migrate_legacy_key\|fn migrate_search_keys" src-tauri/src/bot/config/io.rs` 命中——启动期把老配置明文迁进凭据存储。
  10. 【可核对】`grep -n "KEYRING_SERVICE\|LEGACY_KEYRING_SERVICE" src-tauri/src/bot/config/types.rs` 命中——凭据存储的 service 名有新旧两套，老条目有迁移路径。
- **证据**：截图（厂商条目已存凭据、MCP 机密已存凭据）+ 两条 grep 的终端输出 + `<data_dir>/bot.log` 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（`config.plaintext_key_stripped` / `config.search_key_migrated` / `mcp.secret_migrated` 三个都 grep 得到才算名还在）
- **缺口补验**：遗留 4 条 keyring 条目 ACL 为 -A 待拍板
