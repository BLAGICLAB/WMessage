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
- **未修 0 条**：高危 / P1 / P2 三档全部修完（L1 / L2 / L3 见 §四，U1–U13 见 §五）
- **第三轮**：§五「待复核 U1–U13」已逐条核对——**12 条确认为真并已修**，
  U5 拆两半（删会话那半是真缺陷已修，`/stop` 那半是设计语义不改）。
  逐条结论与修法见 §五，全量门禁结果见 §七

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
| U1 | usage 逐块累加：每块都发 usage 的网关上总量翻倍 | 中 | `129bf03` |
| U2 | `/stop` 未覆盖终稿自检与 replan 两处长 await（最多再烧 180s） | 中 | `231d8961` |
| U3 | Anthropic thinking 块未原样回传：tool_result 轮 400 | 中 | `3c87d87a` |
| U4 | 压缩收缩守卫前算 `total - head_keep - tail_keep`，无符号下溢 panic | 中 | `e5e162b` |
| U5 | 删会话不级联取消在途子 agent：没人收的活烧完整份预算 | 中 | `c98fb6e` |
| U6 | 两个 async 工具函数里跑同步 syscall，阻塞 runtime 线程 | 中 | `690b915` |
| U7 | 读路径 inode 复核与实际读取分属两个 open（复核没管住读到的内容） | 中 | `b0002fc` |
| U8 | `spawn_subagent` 无幂等键：重复调用建两条活行、各拿一份完整预算 | 中 | `88ae51b` |
| U9 | runner panic 跳过整条收尾：令牌泄漏 + 前端该行永远转圈 | 中 | `f956873` |
| U10 | 变更声称守卫认不下「不可核验但确实改状态」的工具 | 中 | `08f4cf0` |
| U11 | `tool.return` 两处早退手搓 kv，将来加键会漏 | 低 | `caabe2d` |
| U12 | 任务工具 schema 指向已下线的 `list_tasks` | 低 | `5708252` |
| U13 | `edit_file` 空白容错重建丢尾换行、混合 EOL 被整体归一 | 低 | `98fd1b2` |

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

## 五、待复核 U1–U13 的核对结论（第三轮逐条核实）

原表是子代理标 NEEDS-VERIFICATION 的线索，**不是结论**。第三轮逐条读过代码、
读过协议文档、写过复现测试再改，结论如下。**12 条确认为真并已修，1 条（U5）
拆成两半**：一半是真缺陷（已修），一半是设计语义（不改，理由写进 `SPEC.md`）。

| # | 核对结论 | 处置 | 提交 |
|---|---|---|---|
| U1 | **真**，但触发面比线索窄：标准 OpenAI/Anthropic 的 `usage` 是**累计总量**（末块即总量），逐块 `+=` 才会翻倍；翻车的是「每块都发 usage」的网关 | 按**字段**取最后一次非零值（`merge_stream_usage`），不整块替换 | `129bf03` |
| U2 | **真**：终稿自检（60s）与 replan（120s）两处 await 之前没有 stop 检查 | 两处各加 stop 检查 + 源码扫描守卫 | `231d8961` |
| U3 | **真**：Anthropic 官方口径要求 assistant 轮原样回传 thinking/redacted_thinking 块，丢块在 tool_result 轮会 400 | 按官方口径原样回传（含 signature），OpenAI 路径剥掉；400 且含 thinking+signature 时删块重试一次 | `3c87d87a` |
| U4 | **真**：`total - head_keep - tail_keep` 在收缩守卫之前算，无符号下溢 | 长度守卫提到所有分支之前（短于 head+tail 本就没得压）+ `saturating_sub` | `e5e162b` |
| U5 | **一半真**：`/stop` 不级联=**设计语义**（非阻塞派发 + 后台汇报轮，`/stop` 只停本会话当前这次交互执行）；**删会话不级联=真缺陷**（产物没人收、行没人看得见，还照烧完整份预算） | `/stop` 不改（口径写进 `SPEC.md`）；删会话连带取消在途子 agent | `c98fb6e` |
| U6 | **真**：两个 async 工具函数里直接跑同步 syscall（含 per-task `canonicalize` 循环），阻塞 runtime 线程 | 搬进 `spawn_blocking_map`，同步内核抽成可被守卫判定的纯函数 | `690b915` |
| U7 | **真**，且比线索记的更实：不是「复核窗口没覆盖读取」，是**复核根本没管住读到的内容**——复核用的 fd 在闭包内 drop，真正读内容时按路径重新 open | 复核、判二进制、读内容合并到**同一个 fd**（一次 open） | `b0002fc` |
| U8 | **真**：`spawn_subagent_locked` 无条件插新行，重复调用建两条活行、各拿一份完整预算 | 服务端幂等键（父会话 + 档位 + 目标 + 验收标准 + 指定模型，只认活着行） | `88ae51b` |
| U9 | **真**：runner panic 会跳过整条收尾——取消令牌不反注册、`subagent-finished` 不发，前端那一行永远转圈 | panic 捕获归一成普通失败 + RAII 反注册守卫 | `f956873` |
| U10 | **真，但影响面比线索写的小**：线索说「`clipboard_write`/`run_python` 标 `mutating: false`」，实测真正缺凭据的是**变更声称**这道闸（落盘凭证闸已经放行过它们） | 把「不可核验但确实改状态」纳入变更声称闸的凭据口径 | `08f4cf0` |
| U11 | **真**（当前键集一致，不是当下的 bug，是将来加键会漏） | 两处早退改走 `tool_return_kv` 构造器 | `caabe2d` |
| U12 | **真**：任务工具 schema 指向已下线的 `list_tasks` | 改 `query_tasks` + 同步 schema 基线 + 退役工具名守卫 | `5708252` |
| U13 | **真**：空白容错分支按行重建全文 → 混合 EOL 被整体归一、`newString` 尾换行丢失 | 改按**字节区间**拼接，区间外内容连其换行原样保留 | `98fd1b2` |

### U1–U13 怎么修的

**U1 usage 逐块累加**（`129bf03`）：新增纯函数 `merge_stream_usage`——按**字段**
取最后一次非零值，而不是逐块 `+=`，也不是整块替换（整块替换会把 Anthropic 的
input 抹成 0）。四个局部变量合并成一个 `TokenUsage` 累加器。测试 3 条，其中一条
专门钉住「Anthropic `message_start` 给 input、`message_delta` 给 output，两个
字段都得留住」。

**U2 `/stop` 覆盖两处长 await**（`231d8961`）：终稿自检（`summarize_http`，60s）
与 replan（planner，60s）前各加一次 stop 检查。接线点用**源码扫描守卫**
`stop_guard_tests` 锁住（前缀 rfind 取最后一次 stop 检查、距离 ≤4000 字节、
中间无 `.await`）——行为级用例测不到「检查点还在不在那个位置上」。

**U3 Anthropic thinking 块未回传**（`3c87d87a`）：按 Anthropic 官方文档「请将
助手轮次原样回传，包括思考块」实现：解析 `content_block_start` 的
thinking/redacted_thinking 与 delta 的 thinking_delta/signature_delta，块随
assistant 消息入历史（放 provider 专属键 `anthropic_thinking`），转 Anthropic
协议时原样回放且排最前，OpenAI 路径剥掉。thinking 块只活在单次 run 内（不落库），
跨 run 无签名失效风险；同 run 内前缀稳定，砍历史导致前缀变化时由「400 且响应体含
thinking+signature → 删块重试一次」兜底。测试 8 条。

**U4 无符号减法下溢**（`e5e162b`）：长度守卫从 `else if` 提到所有分支之前——短于
head+tail 的文本本就没得压，而这道守卫又是 `total - head_keep - tail_keep` 不
下溢的前提；减法改 `saturating_sub` 双保险。红/绿对照过（panic: attempt to
subtract with overflow @ compaction.rs:158）。

**U5 父会话 `/stop` 不级联取消子 agent**（`c98fb6e`）：分两半处理。
`/stop` 那半**不改**——`spawn_subagent` 是非阻塞派发，结果由后台汇报轮回灌父
会话；`/stop` 的语义是「停本会话当前这次交互执行」（`check_subagent` 等待期响应
`/stop` 就是这个意思：停了等待不等于杀了后台活），子 agent 行在聊天内嵌区与右侧栏
可见可单独取消。删会话那半**是缺陷**——父会话没了，产物没人收、行没人看得见，
子 agent 却照烧完整份预算、照落盘写文件；改成删会话时把在途行（`parent_session_id`
或 `session_id` 挂在该会话下的 queued/running 行）一并置 `cancelled`，行留着不删
（runner 收尾还要读它），置终态后 runner 的 2s watcher 见到终态即停工具循环。

**U6 async 里跑同步 syscall**（`690b915`）：`extract_path_check` /
`tool_link_file_to_task` 的 canonicalize/exists/gen_dir 搬进 `spawn_blocking_map`；
同步内核抽成非 async 的 `probe_paths_sync`（返 `PathProbe`）与
`any_bound_path_matches`，让「async 函数体内无裸 syscall」可被守卫判定。报错文案
与判定口径逐条保持原样。守卫 `sync_syscall_off_runtime_tests`。

**U7 读路径 inode 复核不覆盖实际读取**（`b0002fc`）：`is_binary_file_with_ino_check`
+ `read_capped_file` 合并成 `read_with_ino_check`——**一次 open**，同一个 fd 上
stat 比对 inode → 读内容 → 前 8KB 判二进制；inode 不一致时内容一概不返回。顺带
删掉被孤儿化的 async `read_capped_file`。unix 保持 inode 比对，Windows 仍无
TOCTOU 保护（既有设计选择，非缺陷）。测试 3 条 + 接线守卫 2 条（读路径只开一次
文件；守卫已用注入二次 open 实测过会红）。

**U8 `spawn_subagent` 无幂等键**（`88ae51b`）：幂等键由**服务端派生**（不指望模型
给稳定 key）：同一父会话里，档位 / 目标 / 验收标准 / 指定模型全同，且那一条**还
活着**（queued/running）。命中就返回同一个 `subagentId`：不建行、不发事件、
**不再起第二个 runner**（起第二个就是同一份活跑两遍、各烧一份预算）。取舍：已
终态的不参与去重（「再跑一次」是重试语义）；跨父会话不去重；拿不到父会话 id 时
宁可多建一行；键里不含 onion（onion 不落库，加列要动 schema，而「同目标同验收
却换了配置档」几乎不会出现）。回执文案分两种形态——新建说「已派发」，命中幂等键
说「未重复派发」，模型照抄不会把一个子 agent 说成两个。

**U9 runner panic 泄露**（`f956873`）：runner future 包 `catch_unwind()`（bot_scheduler
既有范式），panic 经 `panic_to_command_error` 归一成 `CommandError::Internal` 走
正常收尾（行落 failed、发 `subagent-finished`）；另加 `StopTokenGuard`（Drop 反注册
`subagent_stops`）兜 panic 路径。测试 2 条。

**U10 变更声称守卫拿不到凭证**（`08f4cf0`）：线索指 `clipboard_write`/`run_python`
的 `mutating: false`，实测真正缺凭据的是**变更声称**这道闸（落盘凭证闸已经放行过
它们）。把已有的 `unverifiable_ran`（run_python / mcp_* / spawn_subagent）从落盘
凭证闸**提升**到变更声称闸，抽纯函数 `claim_mutation_fires`。**有意不改
`mutating` 标志**：给 run_python 加 mutating 会触发工具结果自检，自检判「不符预期」
会让模型重跑脚本，副作用翻倍；clipboard_write 没有声称话术，标记只多一轮无意义
自检。测试 2 条。

**U11 `tool.return` 手搓 kv**（`caabe2d`）：预算触顶、入参 JSON 非法两处早退改走
`tool_return_kv` 构造器；守卫扩到三处分支（锚点用事件名，避免自匹配）。

**U12 schema 点名已下线工具**（`5708252`）：`SCHEMA_COMPLETE_TASK` 的 taskId
描述 `list_tasks` → `query_tasks`；同步 `tests/fixtures/tools_baseline.json`；新增
守卫 `retired_tool_names_absent_from_schemas`。

**U13 `edit_file` 空白容错**（`98fd1b2`）：空白容错分支不再按行重建全文，改按
**字节区间**拼接（新增 `line_ranges` / `dominant_eol` / `normalize_eol`）；区间外
内容连其换行原样保留（混合 EOL 不再被整体归一）；`newString` 尾换行保留（与精确
分支 `replacen` 语义对齐）。测试 2 条，红/绿对照过。

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

### 第三轮（U1–U13 逐条核对修复）

§五那 13 条是子代理的线索不是结论。第三轮逐条读过代码、读过 Anthropic 官方
协议文档，**先写复现测试再改**，每条单独提交（12 条修，U5 拆两半修一半留一半）。
修法与取舍逐条写在 §五「U1–U13 怎么修的」。

红/绿对照做了 6 条：U1（改回逐块累加即红）、U2（挪走 stop 检查即红）、
U4（还原下溢即红）、U7（注入二次 open 即红）、U8（摘掉幂等查表即红）、
U13（还原按行重建即红）。U3 与 U6 也做过对照。U9 / U10 / U11 / U12 的新用例
与修复一并写出、修复后转绿，其中 U12 是**既有基线对拍**直接红（`complete_task`
在基线前 28 项内，schema 一改基线就漂）。

接线守卫 6 条（U2 stop 检查位置、U6 async 内无裸 syscall、U7 读路径只开一次
文件、U8 deduped 早退早于 runner 派发、U10 变更声称凭据口径、U12 退役工具名不
出现在 schema）：这类「调用点是否存在 / 接线是否正确」单测验不到，用源码扫描
钉住。

```
cargo nextest run       1764 passed / 7 skipped
cargo test --lib        1587 passed / 5 ignored
cargo fmt --all -- --check ✓
npx tsc --noEmit           ✓
npx oxlint src             ✓（exit 0，仅既有 warning）
npx vitest run             85 文件 / 880 用例全绿
tests-audit/               26 条全过（须逐个点名 pytest 文件，整目录跑会静默放过）
```

> 这几个数是**工作区当前值**，含并发会话同期提交（动效 / 任务卡编辑那批）带来的
> 用例增量，不是本轮 13 条修复的净增量——本轮的净增量见上面「本轮新增测试」
> 一段。

本轮新增测试：`bot_model_loop.rs`（U1 usage 合并 3 条 + U10 变更声称口径 2 条 +
U2 stop 守卫）、`bot_anthropic.rs`（U3 thinking 8 条）、`bot/compaction.rs`
（U4 2 条）、`bot/dispatch.rs`（U11 守卫）、`bot/registry.rs`（U12 退役工具名
守卫）、`bot_fs.rs`（U13 2 条、U7 3+2 条）、`bot/tools.rs`（U6 守卫）、
`bot_orchestrator.rs`（U9 2 条、U8 8 条）、`db/mod.rs`（U5 2 条）。

> U5 的 `/stop` 那半**故意不改**：`spawn_subagent` 是非阻塞派发、结果由后台汇报
> 轮回灌，`/stop` 只停本会话当前这次交互执行。改它等于把「后台干活」这个设计
> 前提推翻。口径已写进 `SPEC.md`，不再是悬案。

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
- §五「待复核 U1–U13」**已逐条核对**（第三轮）：12 条确认为真并已修，U5 拆两半
  （删会话那半已修，`/stop` 那半是设计语义、不改）。原表是子代理的线索不是
  结论——其中 U10 的定位（指向 `mutating: false`）实测有偏差：真正缺凭据的是
  变更声称那道闸，不是落盘凭证闸。
- 剩下三处**有意留白**（都不是本轮该动的）：U3 的「400 + thinking+signature →
  删块重试一次」只兜一次；U8 的幂等键不含 onion（onion 不落库）；U7 在 Windows
  端仍无 TOCTOU 保护（既有设计选择，follow-up 记在 `Phase 6`）。
