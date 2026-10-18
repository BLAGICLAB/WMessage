# Agent 模块全量审计报告（2026-10-19）

范围：`src-tauri/src/` 的 agent 内核——模型循环、工具分发与闸门、工具体、
子 agent 编排，加上 config / memory / skills / scheduler / trace 等外围。
共 71 个 Rust 文件、约 4.2 万行（`bot*.rs` + `bot/**` + `memory/**` +
`bot_skills/**` + `py/**`）。审计对象是 `feat/apps-archived-delete` 分支上
图谱改动落库后的工作区现状。

**结论**：基线健康——`cargo nextest` 1467 用例全绿、SSRF / 权限 / 预算 /
状态机等闸门设计扎实。但查出 **2 条 P0 与 1 条被修复激活的高危**，其中两条
P0 是「文档与系统提示词承诺的能力，代码从不执行」的功能性断裂。

- **已修 2 条**：`3fd06cb`（降级后记账下标越界 panic）、`e8d2270`（引用块标记漂移）
- **未修 1 条高危**：`e8d2270` 让批量执行路由真正可达，同时激活了一条原本
  因路由恒空而休眠的解析缺陷（见 §2），**建议优先处理**
- **未修 4 条 P1 / 3 条 P2**，清单在下面，每条带文件行号与证据

---

## 怎么审的

四个 `explore` 子代理并行，各查一个互不重叠的文件所有权面，全程只读；
主会话另做 compiler 层信号（clippy）与仓库自带一致性门禁
（`tests-audit/`）扫描，并亲自复核 config / memory / scheduler / trace。

| 角度 | 范围 | 条数 |
|---|---|---|
| A 模型循环 | `bot_model_loop.rs` / `bot/compaction.rs` / `bot_anthropic.rs` / `bot_chat.rs` | 8（严重 1 / 高 2 / 中 2 / 低 3） |
| B 分发与闸门 | `bot/registry.rs` / `bot/dispatch.rs` / `bot/hooks.rs` / `bot_onions.rs` / `tool_guard.rs` / `intent_router.rs` | 7（高 3 / 中 3 / 低 1） |
| C 工具体 | `bot/tools.rs` / `bot_fs.rs` / `bot_web.rs` / `bot_py.rs` / `ask_engine.rs` / `bot_desktop.rs` / `ocr.rs` | 7（高 2 / 中 2 / 低 3） |
| D 子 agent 编排 | `bot_orchestrator.rs` / `db/subagents.rs` / `prompts/subagent.rs` / `exec_steps.rs` | 8（高 2 / 中 3 / 低 3） |

**高危条目由主会话逐条复核过代码**，没有只转述子代理结论。凡未亲自确证的，
一律标进 §5「待复核」，不当事实记。

**四路发现的共同形状**：不变式只写在注释里，没有钉在代码或测试上。
`tool_msg_ages` 的「下标恒稳定」、取消门控的「用户取消不汇报」、
`clear_for` 的「结束即复位头像」——三处注释都把某个前提说成了事实，
而真实前提在别的分支上被悄悄破坏了。这类缺陷单测抓不到，
因为测试压根没覆盖那条分支。

---

## 一、已完成

| # | 问题 | 严重度 | 提交 |
|---|---|---|---|
| A#1 | 上下文超限降级砍历史后 `tool_msg_ages` 下标越界 panic；切口还会留下孤儿 tool 消息 | 严重 | `3fd06cb` |
| B#1 | 前端发 `[引用任务]`+ 纯标题，后端认 `[已选任务]`+ `id=` → 批量执行路由恒空 | 高 | `e8d2270` |
| B#2 | 触发词裸 `contains` 匹配：修复 B#1 后 `download`/`google`/`cargo run` 会静默开跑 | 高 | `e8d2270` |
| L4 | 模块树行数标注漂移，pre-push 门禁红 | 低 | — |

> L4 修在承载本报告的那个提交里。提交无法在自身文件内引用自己的 hash，
> 查 `git log` ——按提交信息 `模块树行数标注跟上实际文件` 定位。

### A#1 怎么修的

先写复现测试再改，修复前两条用例红得与预测逐字吻合：

```
降级后记账下标 6 越界：msgs 只剩 6 条
降级后留下孤儿 tool 消息（下标 [2]），发出去必 400
```

两处根因分别修：

- **下标越界**：`shed_oldest_history` 改返回 `(切口下标, 砍掉条数)`，调用方
  同步调新的 `reindex_after_shed`——落在切口内的记账作废（消息已不存在），
  其后的按 `-(removed-1)` 左移（砍 `removed` 条、补回 1 条占位）。
- **孤儿 tool 消息**：切口不能停在 tool 分组中间。新增边界延伸——边界消息是
  `tool` 就连带砍掉（其父 `assistant(tool_calls)` 已落在可砍区间内），
  上界停在 `len-1` 以保住本轮输入及其父消息。

新增 3 条回归用例：记账下标始终可寻址且仍指向 tool、被砍掉的记账不残留、
降级后不产生孤儿 tool。原来那句宣称「msgs 循环内只追加不删、下标恒稳定」
的注释本身是错的，已改成指向这条例外。

### B#1 / B#2 怎么修的

前端改发 `[已选任务]` + `- id=X，标题=Y`，与 `parse_selected_tasks_block`
逐字对齐（`refs` 本就带 id，只是没发出去）。

B#2 必须与 B#1 一起修：路由一旦能触发，原来的裸 `contains` 立刻变成新风险。
ASCII 触发词收紧为「**必须是命令首词且右边界干净**」：

| 输入 | 修复前 | 修复后 |
|---|---|---|
| `download the report` | 触发 | 不触发（`go` 内嵌） |
| `google 一下` | 触发 | 不触发（`go` 内嵌） |
| `cargo run` | 触发 | 不触发（`run` 非首词） |
| `go一下` / `run 一下` / `do it` | 触发 | 仍触发 |

CJK 关键词维持子串命中（中文无词边界）。宁可漏触发——系统提示词规则 17 本就
明令模型「若路由未触发，按规则 1-16 处理，禁止自行声称已开始批量执行」，
漏了是安全方向。

**两端各加回归锁**：Rust 侧用前端真实格式断言能解析出 id，TS 侧断言发出的
`content` 带标记与 id，防止再次各自漂移。

---

## 二、未修 · 高危 1 条（**因 B#1 的修复而激活**）

| # | 问题 | 位置 | 证据 / 影响 | 建议 |
|---|---|---|---|---|
| **N1** | 引用块解析**无结束边界**，用户正文里任何 `- id=X，标题=Y` 行都会被当成待执行任务卡 | `intent_router.rs:56-59` | `for line in after.lines()` 遇到非 `- ` 行是 `continue` 不是 `break`，一路扫到消息末尾。而前端拼的是 ``content = `${attach}${refBlock}${text}` ``（`ChatPanel.tsx:1609`），引用块以 `\n\n` 结尾，**用户自己的正文就在扫描范围内** | 解析止于引用块的连续 `- ` 行（遇空行或非 `- ` 行即停），或由前端把块与正文用不可混淆的分隔符隔开 |

**为什么现在要紧**：`e8d2270` 之前路由恒空，这条缺陷是休眠的；现在解析通了，
它**可达**了。触发条件不需要攻击者——用户引用两张卡、打「完成」，
正文里恰好有一行像任务引用（粘贴日志、从别处复制的 `- id=...` 行、
甚至自己手写）——那张卡就跟着被自主执行。命中语义是「全部按自主执行跑一遍」，
误判代价不低。

---

## 三、未修 · P1 中危 4 条

| # | 问题 | 位置 | 证据 / 影响 | 建议 |
|---|---|---|---|---|
| M1 | Anthropic 协议降级重发必 400：body 是 `build_anthropic_body` 转出的块结构，降级却无条件回填 OpenAI 形状的 `msgs` | `bot_model_loop.rs:1781` | `body["messages"] = serde_json::to_value(&msgs)` 无视 `http.provider`（同函数 :1643 就在用它）。用户还会丢掉「上下文超长，请 /compact」的指引 | 降级重发走与首包同一套协议构造；`msgs` 已是协议中立的唯一源，应由它重建 |
| M2 | 用户取消子 agent 后仍起汇报轮：门控拿的是初始快照，状态恒为 `Queued`，判断永不成立 | `bot_orchestrator.rs:2371` | `spawn_parent_report(&app, &row)` 收 :1564 的初始 `row`，全程只 clone 不回读；而 :1602 保证到达此处时状态**恒为 Queued**。直接违反设计文档 `SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md:193`「用户取消不汇报」。同函数 :2249 的 `effective_status` 已读到真实状态并正确用于事件，只是没传给门控 | 传 `effective_status` 或就地调现成的 `load_subagent_status`（:2501） |
| M3 | 逐步执行**正常做完**最后一步时，任务卡机器人头像永不复位 | `exec_steps.rs:312` | `resume` 在 :382 已 `take_pending_for` 摘走挂起项，:312 的 `clear_for` 里必然拿不到 → :74 的 `set_bot_assigned(false)` 不执行。卡片要等重启才恢复。代码自身注释（:370、:388）证明作者知道这个不对称，并为**错误路径**做了显式复位，唯独漏了成功路径 | 完成分支显式 `set_bot_assigned(app, &task_id, false)` |
| M4 | `edit_file` 先整读入内存、再判 1MB 上限 | `bot_fs.rs:940` | 无界 `read_to_string` 在前，:946 才检查大小。同文件 :1253 的 `read_capped_file_sync` 存在的理由注释写得一模一样：「整读入内存后才截断…会把进程内存打爆」。白名单内一个 GB 级日志即可打爆进程 | 换成 `read_capped_file_sync` |

**M2 与 M3 是本次审计里性价比最高的两条**——都是「把已经算对的值传下去」
的小改动，不涉及新逻辑、不改协议。

---

## 四、未修 · P2 低危 3 条

| # | 问题 | 位置 | 证据 / 影响 | 建议 |
|---|---|---|---|---|
| L1 | `kill_tree` 漏了兄弟函数有的两道 pid 护栏 | `py/runtime.rs:171` | 直接 `kill -9 -{pid}`，既无 `is_live_group_leader` 也无 `pid > i32::MAX` 检查；而 :245-257 的 `kill_py_children` 两个都有，:160-162 注释明确警告缺了会 `kill -9 --1` 误伤全系统。主循环 :791 已 reap 子进程，:830 才调 `kill_tree` | 补齐两道检查，与 `kill_py_children` 对齐 |
| L2 | 一个回归测试被静默禁用：`usage_stats_daily_zero_fills_and_counts_finished_only` **没有 `#[test]`**，是死代码 | `db/trace.rs:1784` | 全仓唯一引用就是它自己，从未被收集运行。它守护的是「窗口缺日补零 + 僵尸 running 不计入执行次数」这套成本统计口径。根因是 `b147bc8` 插入新测试时把旧测试的属性和函数体劈开，留下 :1724 一个游离的重复 `#[test]`（clippy 的 `duplicated attribute` 警告就是它）。**已实证**：默认不收集；临时补上 `#[test]` 后通过 → 是覆盖丢失，不是产品缺陷 | 补回 `#[test]`，顺带消掉 clippy 警告 |
| L3 | 瞬时线程池故障会把授权放大一级 | `bot_fs.rs:292` | `is_dir_async(...).unwrap_or(false)`：用户选目录 → 桥接失败判成文件 → :374 改存**父目录**进 `allowedDirs`。安全闸门朝**过度授权**方向失败 | 失败时不落盘（宁可不授权），或显式区分「不是目录」与「探测失败」 |

### L4 怎么修的

推分支时被 pre-push 拦下才发现**有两条漂移，不是一条**：`lib.rs`（1057→1063，
图谱那批提交留下的既有红灯）与 `bot_model_loop.rs`（4263→4427，差 164 行正好是
`3fd06cb` 的净增）。后者是本次修复自己引入的——审计期间跑这道门禁时两个修复都
还没落地，提交后没有重跑，是漏检。两处一并按实际行数订正。

顺带说一句：这道门禁拦住的是**我自己**，说明它确实在干活。

---

## 五、待复核（未确证，不建议直接照改）

以下条目子代理自己标了 NEEDS-VERIFICATION，或我没亲自读到代码。列出以免
重复劳动，但**动手前请自行确认**。

| # | 线索 | 位置 |
|---|---|---|
| U1 | 逐块累加 usage 与其上方注释「调用方**只认末块**，不能逐块累加」自相矛盾；对标准 OpenAI/Anthropic 正确，但每块都发 usage 的网关会重复计数 | `bot_model_loop.rs:1890` vs `:262` |
| U2 | `/stop` 未覆盖两处长 await（终稿自检 `summarize_http`、replan），各可烧 60s + 120s，UI 停在「收尾中」像是停止失灵 | `bot_model_loop.rs:2386`、`:2733` |
| U3 | 开了 extended thinking 时 assistant 轮不回传 thinking 块，Anthropic 文档称 tool_result 轮缺 thinking 会报错 | `bot_anthropic.rs:370` |
| U4 | `total - head_keep - tail_keep` 在收缩守卫之前计算，无符号减法 | `bot/compaction.rs:158` |
| U5 | 父会话 `/stop` 或会话删除都不级联取消子 agent（子 agent 继续烧到预算耗尽）；设计文档未规定是否该级联，需产品确认 | `bot_orchestrator.rs:1968` |
| U6 | `extract_path_check` / `tool_link_file_to_task` 在 async 函数里直接跑同步 syscall（含 per-task `canonicalize` 循环）；同文件 :86 有正确范式 `spawn_blocking_map` | `bot/tools.rs:1264`、`:1132` |
| U7 | 读路径的 inode 复核不覆盖实际读取（fd 在闭包内 drop，`read_capped_file` 重开路径）；默认读白名单含 `~/Downloads`。作者自己记为「Phase 6」待办 | `bot_fs.rs:1352` / `:1458` |
| U8 | `spawn_subagent` 无幂等键：LLM 重复调用会建两条活行，各拿一份完整预算 | `bot_orchestrator.rs:240` |
| U9 | runner panic 时 `subagent_stops` 条目泄漏（RAII 未覆盖该表）且不发 `subagent-finished`，前端该行永远转圈 | `bot_orchestrator.rs:1950` |
| U10 | `clipboard_write` / `run_python` 标 `mutating: false`，幻觉防护拿不到凭据回执。影响有限——`MUTATING_TOOLS` 不参与任何确认弹窗 | `bot/registry.rs:1360`、`:1228` |
| U11 | `tool.return` 四个早退分支里有两处手搓 kv，未走 `tool_return_kv` 构造器；键集当前一致，但将来加键会漏这两处 | `bot/dispatch.rs:500`、`:545` |
| U12 | 任务工具 schema 指向已不存在的 `list_tasks`（已并入 `query_tasks`）与 `[已选任务]` 块 | `bot/registry.rs:195` |
| U13 | `edit_file` 空白容错重建时丢掉 `newString` 请求的尾换行；混合 EOL 文件被整体归一 | `bot_fs.rs:747` |

---

## 六、已核验、确认没问题的部分

别再查了，这些都经过严格核验：

**网络安全**
- **SSRF 防护无懈可击**（`bot_web.rs`）：主机名 + 每个 DNS 解析 IP 双重校验、
  结果 pin 住不二次解析、客户端禁重定向 + 逐跳重验（含 Location 目标）、
  拒绝 https→http 降级、十进制/十六进制/八进制 IP 字面量先解码再判段、
  Jina 兜底对私网目标不可达

**权限与写入**
- 写白名单确实不含 Desktop/Downloads/Documents 三个读白名单默认项
- 「始终允许」授权粒度取用户原始输入的父目录，**不用** `canonical(parent())`
  （后者可被 symlink 劫持指向白名单外），有测试锁死
- 非交互执行永远到不了授权弹窗（deny-first）

**子 agent 隔离与预算**
- 三硬顶全部服务端强制：轮数在循环体内判、工具调用走原子 `fetch_update`
  （执行前扣、至多一次、无退款路径）、墙钟 `timeout` 包住整循环且排队期不计时
- 并发闸无泄漏面：`GateTicket`/`WaitingGuard` 全 RAII Drop，无跨 `.await` 持锁
- 状态机无旁路：全仓仅 `db/subagents.rs:480` 一处 `UPDATE subagents SET status`
- 子 agent 上下文 fail-closed：RAII guard 的 Drop 在同一把锁内复核归属，
  工具层再复核一次（`bot_orchestrator.rs:2737` 无 ctx 即报错）
- `check` 是纯读幂等，重复调用不双计

**工具注册表**
- 45 条表项 = `BUILTIN_TOOL_COUNT`，`name` 与各自 `SCHEMA_*` 的
  `function.name` 逐条相符，无重复，`call_*` 全部存在且调用匹配实现
- MCP 工具名恒为 ASCII `mcp_<server>_<tool>`（hash 钳制 + `_2` 去重），
  不会遮蔽内置工具，`tools_index` miss 的反查兜底无法劫持

**闸门顺序与编码细节**
- 实际顺序与架构文档一致：L4 钩子 → 规则闸 → `skill_on_step` → 子 agent
  白名单 → 预算 → JSON 闸 → 分发，无早退绕过白名单
- `run_pre_tool` 的 fail-open 只对 `Err`/panic 生效，**deny 不会被吞**
- 字符边界切片全部走 `chars()`，无字节下标 panic
- `ChatGuard`/`ExecGuard` 是 RAII，任何早退都会释放，不存在卡死「运行中」
- 工具调用累积器：index-0 默认、Anthropic 块号走稠密槽映射、缺 name 的
  幽灵条目丢弃、合成 id 按 index 唯一、中途 abort 会回填占位 tool 响应

**配置与密钥**
- 密钥走 keyring，`skip_serializing` 收口到「配置永不明文」；
  Linux 降级单文件 0600 原子写（有残渣测试）
- 配置写 tmp + fsync + rename，失败清残渣

---

## 七、验证记录

P0-1 的复现做过**红/绿对照**：修复前两条新用例红（报错文本与审计预测逐字
一致），修复后转绿。P0-2 的两条新用例同理。`db/trace.rs` 那个死测试
（L2）也实证过：默认不进收集集；临时补 `#[test]` 后**通过**——证明是覆盖
丢失而非产品缺陷（该临时改动已还原，未留在提交里）。

```
cargo nextest --lib   1473 passed / 5 skipped（审计前 1467，新增 6）
npx tsc --noEmit      ✓
npx oxlint src        ✓（exit 0，仅既有 warning）
npx vitest run        82 文件 / 810 用例全绿
```

推分支时 pre-push 的 `tests-audit/` 拦下一处 L4 漂移（见 §四 L4），订正后四道
门禁全绿。这也是本报告唯一一处「先写文档、后被事实推翻」的条目——原文写的是
「与本次两个修复无关」，实际上两条漂移里有一条正是 `3fd06cb` 自己引入的。

新增测试：`bot_model_loop.rs` 3 条（shed 下标可寻址 / 被砍记账作废 /
不产生孤儿 tool）、`intent_router.rs` 3 条（英文内嵌词不触发 / 首词仍触发 /
前端真实格式可解析）、`ChatPanel.test.tsx` 1 条断言（发出的 content 带
标记与 id）。

---

## 八、遗留

- **N1 建议下一个做**。它是本次修复带来的可达性风险，且修起来很浅：
  解析止于引用块的连续 `- ` 行即可，与前端 `\n\n` 的既有分隔天然吻合。
- M1 与 `3fd06cb` 在同一条降级路径上，A#1 修完后这条路径的 panic/400 没了，
  但 Anthropic 用户仍拿不到降级效果——**A#1 的收益目前只覆盖 OpenAI 协议**。
- L2 与 L4 是「门禁形同虚设」类问题的两个样本：一个测试静默不跑、一个门禁
  静默不绿（L4 已修，但它的成因还在——行数标注靠人肉同步，改大文件就会
  再漂一次）。建议在 pre-push 里加一条「用例收集数不得下降」的对拍，
  成本极低，能挡住 L2 这类回归。
