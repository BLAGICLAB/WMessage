# Agent 模块全量审计报告（2026-10-19）

范围：`src-tauri/src/` 的 agent 内核——模型循环、工具分发与闸门、工具体、
子 agent 编排，加上 config / memory / skills / scheduler / trace 等外围。
共 71 个 Rust 文件、约 4.2 万行（`bot*.rs` + `bot/**` + `memory/**` +
`bot_skills/**` + `py/**`）。审计对象是 `feat/apps-archived-delete` 分支上
图谱改动落库后的工作区现状。

**结论**：基线健康——`cargo nextest` 1467 用例全绿、SSRF / 权限 / 预算 /
状态机等闸门设计扎实。但查出 **2 条 P0 与 1 条被修复激活的高危**，其中两条
P0 是「文档与系统提示词承诺的能力，代码从不执行」的功能性断裂。

- **已修 5 条**：`3fd06cb`（降级后记账下标越界 panic）、`e8d2270`（引用块标记漂移）、
  N1（引用块无结束边界，详见 §一）、M1（Anthropic 降级重发 400，详见 §一）、
  M2（取消后仍起汇报轮，详见 §一）、M3（逐步执行做完不复位头像，详见 §一）、
  M4（edit_file 整读入内存，详见 §一）
- **原「未修 1 条高危」已修**：N1 因 `e8d2270` 让批量执行路由真正可达而激活，
  本轮连同 B#1 的遗漏（触发词位置与前端相反 → 路由至今恒空）一并修掉
- **未修 3 条 P2**，清单在下面，每条带文件行号与证据

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
| N1 | 引用块解析无结束边界 → 用户正文里的伪引用行被当成待执行任务卡 | 高 | 见下方「N1 怎么修的」 |
| M1 | Anthropic 协议降级重发必 400（OpenAI 形状的 msgs 被回填进 Anthropic body） | 中 | 见下方「M1 怎么修的」 |
| M2 | 用户取消子 agent 后仍起汇报轮（门控读的是行快照，恒为 Queued） | 中 | 见下方「M2 怎么修的」 |
| M3 | 逐步执行正常做完最后一步时，任务卡机器人头像永不复位 | 中 | 见下方「M3 怎么修的」 |
| M4 | `edit_file` 整读入内存后才判 1MB 上限 | 中 | 见下方「M4 怎么修的」 |
| L1 | `kill_tree` 无条件 `kill -9 -{pid}`，pid 回绕即误杀本用户全部进程 | 低 | `15a7481` |
| L2 | 一个回归测试被静默禁用（缺 `#[test]`），从未被收集运行 | 低 | `1058e272` |
| L3 | `is_dir_async` 把探测失败吞成「不是目录」，授权粒度朝宽的方向落盘 | 低 | `c7efaef` |
| L4 | 模块树行数标注漂移，pre-push 门禁红 | 低 | — |

> L4 修在承载本报告的那个提交里。提交无法在自身文件内引用自己的 hash，
> 查 `git log` ——按提交信息 `模块树行数标注跟上实际文件` 定位。
> 第二轮 M1 提交又让 `bot_model_loop.rs` 漂了 36 行，由 `4feb67c` 订正——
> 同一道门禁、同一个成因（行数标注靠人肉同步）。

### M1 怎么修的

请求体构造抽成 `build_request_body`（协议分支 + 采样参数 + 推理字段），
降级重发不再就地回填 `body["messages"]`，而是**整个 body 重建**。

复现（Anthropic + 超长 400 → 砍历史 → 重发）：重发的 body 里是
`{"content":"系统提示词","role":"system"}` —— OpenAI 形状。Anthropic 不认
`role=system`（system 走顶层字段）、不认 `role=tool`（tool_result 块），
故必 400；且那个 400 文案不像超长，`is_context_overflow` 判不出来，用户
连「请 /compact」的指引都拿不到——A#1 修完后这条路径的收益其实只覆盖了
OpenAI 协议。新增集成用例锁住重发体的形状（content 必须是块数组、system
在顶层、旧历史已砍）。

### M2 怎么修的

汇报轮的门控此前读的是**行快照**的 `row.status`——而行是派发时建的，
收尾前 status 恒为 `Queued`，所以「用户取消不汇报」这道门永远判不成立，
用户点过停止后照样起一轮汇报（直接违反设计文档「用户取消不汇报」）。

改为传收尾时算出的**真实终态** `effective_status`（与上报事件、教训沉淀
用的是同一个值），门控收成纯函数 `should_report_to_parent`。另加接线守卫：
单测验不到调用点传的是哪个值，故扫源码钉住「调用必须传 effective_status」
且汇报轮函数体内不得再出现 `row.status`（别处读行快照合法，如
`read_own_card` 读的是库里当前行）。

### M3 怎么修的

收尾分支显式复位机器人头像。原先靠 `clear_for` 复位，而它是「从挂起表取到
pending 才复位」——`resume` 已把 pending 取走（`start` 路径则还没 park），
必然取不到 → 卡片一直顶着机器人头像，直到重启。错误路径早有显式复位
（`start` 的早退、`resume` 的 cleanup_on_err），唯独成功路径漏了。

锁只能扫源码：逐步执行整条链绑在具体 Runtime 上（`AppHandle` 默认 Wry，
mock runtime 驱不动），行为级用例跑不了——而「调用点被删掉」这类变异，
别的用例照样全绿。

### M4 怎么修的

`edit_file` 改成先按上限读、再判体积（复用同文件既有的 `read_capped_file_sync`
——它存在的理由注释写得一模一样：「整读入内存后才截断…会把进程内存打爆」）。
白名单内一个 GB 级日志就能把进程内存打爆。

顺带修掉一处误导：超大文件此前整读时先撞上非 UTF-8 报错，用户看到的是
「不是文本」，看不到真因（文件太大、请拆分）。现在报体积超限，并带上实际
字节数。新增用例：尾部带非法字节的超大文件，报错必须是「超过 edit_file 上限」。

### N1 怎么修的

引用块解析改成认「消息头的结构化块」：顺序固定 `[附件文件]`? → `[已选任务]`? →
用户正文，块本体是连续的 `- ` 行，**到第一行非 `- ` 为止**。两条各挡一类误伤：

- **块无结束边界**：正文里形似 `- id=…，标题=…` 的行（粘贴的日志、手写说明）
  会被收进块，跟着被自主执行；
- **只认消息头**：标记出现在正文中间（用户写的、粘贴的）一律不认。

**核对时另发现 B#1 其实没修好**：前端把块拼在**正文前面**（`ChatPanel.tsx` 的
`content = 附件块 + 引用块 + 用户正文`），而触发词只在标记**之前**找——`user_cmd`
恒为空 → `is_chat_execute_trigger` 恒 `None`，批量执行至今是死代码，审计把 B#1
记成「已修」是错的。B#1 新增的「前端真实格式」回归锁用的也是关键词在前（前端
从不这么发），锁不住漂移。

本次一并修掉：触发词改从**块之后的用户正文**里找（前端真实格式），并把两端
的回归锁换成真实拼装形态（Rust 侧夹具 = 块在前，TS 侧断言块在 `content` 开头、
用户正文在块之后）。附带好处：块之前只有附件块，附件路径里的「完成」「run」
等字样不再被当命令。

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

## 二、未修 · 高危 0 条

原 N1（引用块解析无结束边界）已修，见 §一。

---

## 三、未修 · P1 中危 0 条

（原 M4 已修，见 §一）

## 四、未修 · P2 低危 0 条

L1 / L2 / L3 均已修（L2 由 `1058e272` 补回 `#[test]`，本轮未改代码）。

### L1 怎么修的

原写法是无条件 `kill -9 -{pid}`。pid 是 u32，直接转 i32 回绕成负数时，
`kill -9 --1` 打到本用户**所有**进程；且组号可能已被别的进程组占用
（pid 回收），杀错组等于杀无关进程。

改法是加 `process_group_alive(pid)`：用 `kill(-pgid, 0)` 探组是否还在，
pid 0（kill 语义里是「调用方自己的组」）与超 i32::MAX（回绕）一律拒判；
`kill_tree` 先过护栏才发信号。

**有意偏离本报告的建议**：建议里写的是「补齐 `is_live_group_leader` 检查」，
但那条判据问的是**组长还在不在**，而主循环已 reap 子进程——组长已死时
`getpgid` 失败，只问组长会让「孙进程占住管道」那条路径再也杀不掉进程组，
而那正是 `kill_tree` 存在的理由。故改问「**组还在不在**」。

测试 3 条：危险 pid（0 / 超上限）一律拒判；本进程所在组必判为在（防把
「自己人」也拒掉）；接线守卫扫源码钉住「杀组前必过护栏、pid 不得直接
转 i32」。

### L3 怎么修的

`is_dir_async` 改返回三态 `Option<bool>`：`Some(true)` 是目录 /
`Some(false)` 不是目录 / `None` 探测失败（io::Error 或线程 join 失败）。
原写法把错误吞成 false，调用方分不清「不是目录」与「没探到」。

关键在授权粒度那处：落盘就得先定粒度，而两种粒度此刻都只是猜——猜成
文件会把**父目录**（比用户选中的宽一层）写进白名单，安全闸门朝过度授权
方向失败。故探测失败时**不落盘**：记 `bot_fs.always_persist_skip` 审计，
并明确回报「本次授权未保存（探测失败，请重试）」。宁可不授权，也不朝宽的
方向失败。

三处读路径（`read_text_file` / `grep_files` / `list_files`）同步区分
「不是目录」与「探测失败」——后者要明说请重试，别让用户以为路径类型
没问题、转而去改别的。

测试 2 条：三态区分（目录 / 文件 / 探不到）；接线守卫钉住 Always 分支
必显式处理 `None` 且不落盘。

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

### 第二轮（按 P1→P2 逐条核对修复）

N1 / M1 / M2 / M3 / M4 / L1 / L3 逐条「先写复现测试再改」，每条单独提交。
M2 / M3 / L1 / L3 的接线点用**源码扫描守卫**锁住（纯函数用例测不到调用点
是否存在；M3 那条链绑死在 Tauri 默认 Wry runtime，mock runtime 驱不动，
行为级用例不可行——这是已知取舍，已写进本节）。

```
cargo test --lib           1553 passed / 5 ignored（审计前 1467）
cargo clippy --all-targets exit 0（仅既有 warning，非本轮引入）
cargo fmt --all -- --check ✓
npx tsc --noEmit           ✓
npx oxlint src             ✓（exit 0，仅既有 warning）
npx vitest run             全绿
tests-audit/               26 条全过（须逐个点名 pytest 文件，整目录跑会静默放过）
```

本轮新增测试：`bot_model_loop.rs` 1 条集成（Anthropic 超长重发体形状）、
`bot_orchestrator.rs`（汇报轮门控）、`exec_steps.rs`（头像复位接线守卫）、
`bot_fs.rs` 3 条（超大文件按体积拒 / 探测三态 / Always 分支不落盘）、
`py/runtime.rs` 3 条（危险 pid 拒判 / 本进程组必判在 / 杀组前必过护栏）、
`intent_router.rs`（引用块边界与触发词位置）、`ChatPanel.test.tsx`（块在
`content` 开头、正文在块之后）。

> N1 的改动在提交时被**另一并发会话**的提交卷走（我的 4 个文件混在那个
> 提交里，提交信息对不上）。未重写历史以免干扰对方——代码本身已入库，
> 改动内容见 §一「N1 怎么修的」。

---

## 八、遗留

- **N1 已修**（本轮），M1 亦已修——A#1 的降级路径现在对 OpenAI 与 Anthropic
  两个协议都生效，不再只覆盖 OpenAI。
- 引用块协议已被**重新定义**为「消息头结构化块 + 块之后的用户正文」，两端
  （前端拼装 / 后端解析）逐字对齐，见 `SPEC.md` 对应小节。
- L2 与 L4 是「门禁形同虚设」类问题的两个样本：一个测试静默不跑、一个门禁
  静默不绿（L4 已修，但它的成因还在——行数标注靠人肉同步，改大文件就会
  再漂一次）。建议在 pre-push 里加一条「用例收集数不得下降」的对拍，
  成本极低，能挡住 L2 这类回归。
- §五「待复核 U1–U13」尚未逐条核对——那是下一轮的活。
