# Batch D 重审 + 修复报告（2026-08-19）

范围：`src-tauri/src/middleware.rs`（270 行全读）、`src-tauri/src/profile.rs`（613 行全读）、`src-tauri/src/audit.rs`（172 行全读），交叉核对 `bot.rs` / `bot_chat.rs` / `bot_py.rs` / `lib.rs` / `db.rs` / `api_handlers.rs` / `migration.rs` 调用点。基线 commit：`885cd01`。

## 阶段 1：审计结果

### 已知 D1-D4 + P2 状态复核

**11 条全部仍在，无一条被修**（与 Batch B「全部已修」情况不同）：

| ID | 状态 | 当前位置 |
|---|---|---|
| D1 IntentRouter 恒 Some 短路 pre_step 链 | ❌ 仍在 | middleware.rs:123-126（`Some(route_user_input(input))`，注释仍写「PassThrough 也算 Some」） |
| D2 安全闸门 fail-open | ❌ 仍在 | middleware.rs:98-111（`try_state` 拿不到 → 静默 None；lib.rs:215 已 manage，但结构性 fail-open 未变） |
| D3 profile 非原子写 + 静默重置 | ❌ 仍在 | profile.rs:85-88（`fs::write` 直写）+ 78-83（`unwrap_or_default()`） |
| D4 read-modify-write 无锁 | ❌ 仍在 | profile.rs:173/233/268，无 Mutex |
| P2-13 无 catch_unwind | ❌ 仍在 | registry `run_pre_step`/`run_pre_execute` 直接调 `dyn Middleware` |
| P2-14 双 Vec 分离注册 | ❌ 仍在 | middleware.rs:33-36 |
| P2-15 审计写失败全静默 | ❌ 仍在（且比原审计更差） | audit.rs:71-85：open fail return / writeln `let _`；**`data_dir(app)` 每条事件调 2 次**（71 行 rotate 一次、72 行 open 一次）= 每事件 2 次磁盘写探测，原审计只算了 1 次 |
| P2-16 kv 不转义 | ❌ 仍在 | audit.rs:82-84 + bot.rs:229-238（`truncate_for_log` 不去 `\n`）；bot.rs:287/347 args/preview、bot_chat.rs:340 用户输入均原样落盘 |
| P2-17 读头像无大小上限 | ❌ 仍在 | profile.rs:106-115 `std::fs::read` 不设限 |
| P2-18 先删旧头像再拷贝 | ❌ 仍在 | profile.rs:223-232 |
| P2-19 目录解析两处拷贝 | ❌ 仍在 | profile.rs:55-68 vs db.rs:50-63（probe 文件名、回退链完全一致，目前无 drift） |

补充复核（Batch C 修复未回退）：

- NEW-C-2 锁纪律完好：bot.log 的全部三个写者（audit.rs:70、bot.rs:213、bot_py.rs:318）都持 `BOT_LOG_LOCK`；`api_handlers.rs:216`（api.log）与 `migration.rs:621`（migration.log）写的是另外的文件，不在该锁域内，无交叉。

### 新发现列表（按严重度排序）

全部为 P2，无新 P0 / P1。

| ID | 位置 | 角度 | 严重度 | 一句话问题 |
|---|---|---|---|---|
| NEW-D-1 | bot.rs:282-307 vs 343 | audit 事件顺序 | P2 | 两条早退路径（pre_execute 拦截、skill_on_step 报错）发了 `tool.call` 却永远不发 `tool.return`，统计面板出现「悬挂调用」 |
| NEW-D-2 | profile.rs:247-251 | 头像回退 | P2 | set_avatar 的 save 失败回滚 `remove_file(dest)` 会误删「同扩展名的在役头像」（dest 即当前文件，已被 copy 覆盖） |
| NEW-D-3 | profile.rs:274-277 | 头像回退 | P2 | remove_avatar 先删文件后 save，save 失败 → 磁盘 json 悬挂引用已删文件 |
| NEW-D-4 | profile.rs:106-115 | 数据完整性 | P2 | entry_view 不校验 avatar 字段是否纯文件名，手改 profile.json 可路径穿越读任意文件并 base64 广播 |
| NEW-D-5 | audit.rs:52-60 vs 80-84 | 测试 fixture 一致性 | P2 | `format_event_line` 是生产 `write_event` 行格式的测试专用拷贝，测试全测拷贝、生产拼装零覆盖，drift 时测试照样绿 |
| NEW-D-6 | middleware.rs:17/98/106 | 横切 / 测试覆盖 | P2 | helper `run_pre_step`/`run_pre_execute` 写死 Wry `AppHandle`，与文件头「泛型 Runtime 适配 mock_runtime」注释自相矛盾；state 缺失回退分支（D2 触发点）无法单测 |

### 新发现详情

#### NEW-D-1（bot.rs:282-307 vs 343）

- **症状**：`execute_tool` 在第 0 步发 `tool.call` 审计后才跑 pre_execute 拦截（294-302）和 `skill_on_step`（305-307）。这两条 return 路径都不写 `tool.return`。全仓库 `tool.return` 只有 bot.rs:343 一处（已 grep 确认）。
- **影响**：凡被 AtomicGuard 拦截的调用、以及熔断/计数超限被 `skill_on_step` 拒绝的调用，在 bot.log 里只有 `tool.call` 没有 `tool.return`——按 call/return 配对做的耗时/失败率统计分母失真，表现为一批「永不返回」的悬挂调用。且 skill_on_step 错误路径连一条 Warn 事件都没有（deny 路径至少有 `pre_execute.deny`）。
- **根因**：`tool.call` 发在管线的「拦截段」之前，而 `tool.return` 只在happy path 末尾发。
- **修复策略**（留给后续 batch）：方案 A——`tool.call` 挪到拦截段之后，deny/skill_on_step 拒绝各自已有/补一条独立事件；方案 B——两条早退路径补发 `tool.return`（带 `denied=true` / `reason` kv）。A 更干净，B 改动最小。
- **与已知条目关系**：P2-13 讲的是 panic 路径跳过 tool.return；本条讲的是非 panic 的正常早退路径，原审计未列。

#### NEW-D-2（profile.rs:247-251）

- **症状**：`profile_set_avatar` 在同扩展名覆盖场景（旧 `avatar-user.png` → 新 png）：dest == 在役头像文件，`fs::copy` 已就地覆盖其内容；随后 `save_data` 失败时回滚分支 `remove_file(&dest)` 把在役头像文件删掉，而磁盘上的 profile.json 仍引用该文件名 → 头像静默丢失。
- **根因**：「保存失败删 dest 不留孤儿」的回滚（二次审计 P3 引入，见 247 行注释）没区分「dest 是新孤儿」（异扩展名）与「dest 是在役文件」（同扩展名覆盖）两种情形。
- **修复策略**：回滚前比较 `dest.file_name()` 与改动前 json 里的旧 avatar 文件名，仅当不同（真孤儿）时才删；或干脆 copy 到临时文件名、save 成功后再 rename/清理。
- **与已知条目关系**：P2-18 讲「copy 失败 → 旧头像已删」；本条是「copy 成功、save 失败 → 回滚误删在役文件」，方向相反，原审计未列。

#### NEW-D-3（profile.rs:274-277）

- **症状**：`profile_remove_avatar` 先 `remove_file` 再 `save_data`；`entry.avatar.take()` 只改内存副本，save 失败返回 Err 时磁盘 json 仍引用已被删除的文件 → 悬挂引用（view 静默回落 None，前端表现为「没删掉」但文件已丢）。
- **修复策略**：先 save（内存副本已 take），save 成功后再删文件；删文件失败仅留孤儿文件，无害。
- **与已知条目关系**：P2-18 同家族（写操作顺序不原子）在 remove 路径的实例，原审计只点了 set 路径。

#### NEW-D-4（profile.rs:106-115）

- **症状**：`entry_view` 直接 `profile_dir(app).join(f)`，`f` 来自 profile.json。写入端（set_avatar）只写 `dest.file_name()` 所以正常路径安全，但 profile.json 落在便携目录、可被手改或被损坏写入（如 `../../wmessage.db`），读路径无任何校验就把任意文件读进内存 base64 后广播给所有窗口。
- **根因**：读路径信任了持久化数据，缺「纯文件名」校验。
- **修复策略**：`entry_view` 里校验 `f` 不含路径分隔符 / `..`（或 `Path::new(f).file_name() == Some(f)`），不合规按 None 处理。
- **严重度说明**：攻击者需已有数据目录写权限（同用户权限），故定 P2 防御纵深而非 P1。

#### NEW-D-5（audit.rs:52-60 vs 80-84）

- **症状**：`format_event_line`（带 `#[cfg_attr(not(test), allow(dead_code))]`）是 `write_event` 内联行拼装（80-84 行）的逐字拷贝。audit.rs 全部 3 个格式相关测试测的是拷贝；生产 `write_event` 的拼装逻辑零测试覆盖。两处一旦 drift（比如给生产端加 escaping 忘了同步拷贝），测试照样全绿——等于没有测试。
- **修复策略**：`write_event` 复用 `format_event_line`（时间戳改为参数传入），单一拼装点；顺带给 NEW-D-5 无关的 P2-16 修复铺好单点。
- **与已知条目关系**：P2-16 讲不转义；本条讲格式逻辑双份拷贝导致测试失真，横切「fixture 与生产代码一致性」角度，原审计未列。

#### NEW-D-6（middleware.rs:17/98/106）

- **症状**：文件头注释（17 行）声称「泛型 Runtime 以适配 mock_runtime 集成测试」，但两个 helper（98/106）签名写死 `&tauri::AppHandle`（Wry）。结果：registry 层可单测（现有 7 个测试），helper 层——包括 D2 的 fail-open 回退分支——无法用 mock runtime 覆盖，而 helper 恰恰是生产唯一调用路径。注释与实际行为矛盾，误导后续维护者。
- **修复策略**：helper 改 `<R: Runtime>` 泛型（与 profile.rs 同模式），补「state 未 manage → None」与「state 已 manage → 命中」两条 mock runtime 测试。注意这会把 D2 的 fail-open 行为钉进测试，适合与 D2 修复同 batch 做。
- **与已知条目关系**：D2 讲 fail-open 本身；本条讲该分支不可测试 + 注释失实，原审计未列。

## 阶段 2：实际修复汇总

**无修复、无 commit。** 6 条新发现全部定级 P2，按任务规则「P2 → 只列报告，不修」；未发现新 P0 / P1。

| ID | 严重度 | commit hash | 改了哪几行 | verify 结果 |
|---|---|---|---|---|
| —（无修复） | — | — | — | — |

## 阶段 3：未修的（留到下个 batch）

| ID | 严重度 | 一句话问题 | 建议修复策略 | 估计工期 |
|---|---|---|---|---|
| NEW-D-1 | P2 | 早退路径 tool.call 不配平 tool.return | tool.call 挪到拦截段后，或早退路径补 tool.return | 0.5 天（含配对测试） |
| NEW-D-2 | P2 | save 失败回滚误删同扩展名在役头像 | 仅当 dest 是真孤儿才删；或临时名 + rename | 0.5 天（与 P2-18 同 batch 做更省） |
| NEW-D-3 | P2 | remove_avatar 先删文件后 save | 先 save 后删文件 | < 0.5 天（与 NEW-D-2 同改） |
| NEW-D-4 | P2 | entry_view 不校验 avatar 文件名，可路径穿越 | 读前校验纯文件名，非法按 None | < 0.5 天 |
| NEW-D-5 | P2 | format_event_line 与 write_event 拼装双份拷贝 | write_event 复用 format_event_line，单点拼装 | < 0.5 天（建议在 P2-16 修复前先做） |
| NEW-D-6 | P2 | middleware helper 写死 Wry，D2 回退分支不可测 | helper 泛型化 + mock runtime 测试 | < 0.5 天（建议与 D2 同 batch） |

## 总体 verify

- `cargo build`：✅ 干净，仅 2 个 pre-existing warning（`JournalEntry` dead_code 等，基线 `885cd01` 一致，非本次引入——本次无代码改动）
- `cargo test --lib`：237 passed / 2 failed，2 个 fail 为预期的 pre-existing（`scan_all_skills_in_debug_dir_parse_correctly` + `smoke_all_real_skills_run_dsl_loop_with_mock_executor`，C 路径 DSL 迁移遗留，不在范围）
- 净新增测试数：0（无修复）

## Commit 清单

无（本次无代码修复）。报告文件：`docs/BATCH-D-REAUDIT-2026-08-19.md`（未跟踪，未提交）。
