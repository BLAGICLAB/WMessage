# D1 / Item A：OCR review 失败模式诊断与决策（2026-09-21）

## 结论摘要

四类现象里**只有 1 类是真失败**，且失败**发生在调用侧**（不是 OCR 工具）。
但「调用侧」的**具体机制有一处待确认**（见「机制判定」），故修复方案按
「**精确到调用方 `timeoutSeconds` 参数** + 保留兜底」定，**不**写成笼统的「换后台 / 取消上限」。

## 证据（固化于 `~/.openclaw/cache/ocr-C2b-1-evidence/`）

| # | 现象 | 出处 | 判定 |
|---|---|---|---|
| 1 | `Round 2 failed ... context canceled` + **无产物** | C2b-1 首次 `faint-wi`；17:12:51 收到 SIGTERM；耗时 ~11.7min | **真失败**（全批唯一无产物的一次） |
| 2 | 同批所有 **<10min** 轮次**全部成功出产物** | `marine-trail` 7.6 / `swift-orbit` ~6 / `tidy-glade` ~5 / `good-comet` ~2.5 / `amber-canyon` ~5 min | 反证：失败与**耗时**强相关 |
| 3 | `Round N/2 ... stopping early` | **每次成功轮都有** | **非失败**：OCR Round-2 优化 = **成功**信号 |
| 4 | `code_comment failed: 'comments' array is required` | C2b-2 r2 log:86 | 自愈 |
| 5 | `LLM grouping failed ... falling back to per-file dispatch` | C2b-2 r2 log:4 | 自愈（有设计好的 fallback） |
| 6 | `file_read failed: .../TodoCard.tsx not found` | C2b-1 首次 log:65 | 自愈（路径猜错，后读到正确路径） |

## 机制判定（C2c Step 1 已查实）

- **#1 的 SIGTERM 来源：已查实 = 调用侧 exec `timeoutSeconds`**（2026-09-21 C2c Step 1）。
  - **决定性证据（时间戳对齐到秒）**：启动该 review 的 exec 调用
    `ts=2026-09-21T09:02:51.423Z, timeoutSeconds=600`；SIGTERM `09:12:51Z`；
    差值 **599.577s**（恰等 600s）→ 非巧合。
  - **排除系统 reaper**：同一 agent 会话在 SIGTERM 前后正常继续
    （trajectory seq102@09:12:40 → seq103@09:12:58 → model.completed@09:13:23 → session.ended@09:13:23）。
  - **排除 harness hard deadline**：全库 grep `hard deadline` 命中的均为别的命令（cargo 子进程清理），
    无一与 `faint-wi` 相关。
  - `context canceled` = 次级症状（产物只在结尾写；SIGTERM 中止在飞 LLM 请求）。
  - → Step 2 形态 = **调用侧调参收工**（不需防收割）。
  - 证据路径：`~/.openclaw/cache/ocr-C2b-1-evidence/step1-sigterm-source.txt`；原始记录
    `~/.openclaw/agents/main/agent/openclaw-agent.sqlite` → `transcript_events` seq=2644 +
    `trajectory_runtime_events`（sessionId `d20b502c…`）seq 92–107。
- **`context canceled` 不是独立问题**：它是进程被 SIGTERM 时**在飞 LLM 请求中止**的次级症状。
- **#4 / #5 / #6：独立自愈噪音**（各有 fallback 或非致命），**不产生失败轮次**。
- **#3：非失败**，必须记为成功，否则会误触发 D。

## 决策（三选一）：修 —— 精确到**调用方 `timeoutSeconds` 参数**，不改 OCR 工具代码

**不变量（对齐诊断）**：

- **上限 = 兜底**（防「进程挂死、既无 exit 也无产物」）；**不是根因**。
- **无上限（`0`）= 放弃兜底换时长 → 不采**。
- **后台 vs 前台不是关键变量**；`timeoutSeconds` 的**值**才是。

**最小修复（C2c Step 2 落地）**：

1. `ocr review` 调用侧 `timeoutSeconds` 设**明确高值 `1800`（30min）**——**不设 `0`**。
   覆盖正常 review 耗时（~2–12min）且**保留挂死兜底**。
   （落地形态：**仓库内无 OCR 调用包装脚本**（已核 `scripts/`，只有 test-all/test-fast/install-hooks 等），
   故该参数由**调用侧直接传递**；其**标准值即本文档的 SOP** —— 未来 OCR review 一律
   `timeoutSeconds=1800` + 显式 `--output <每轮唯一路径>`。）
2. ~~待确认分支~~ **已排除**：Step 1 查实 SIGTERM 来自调用侧 `timeoutSeconds`（见「机制判定」），
   **不需要**防收割；harness hard deadline 假设不成立。
3. **失败判据固化（SOP）**：成功 = exit 0 **且** 产物存在且 >0 字节 **且** log 无 `Round N failed` / `context canceled`；
   「stopping early」= **成功**，不触发 D；自愈噪音（#4/#5/#6）**可忽略**；**失败 → D，禁止从 stdout 捞 findings**。
4. 「中途被杀 → 产物全丢」的根治（OCR 侧增量 / 心跳写产物）= **工具改动**，
   登记 follow-up（`C1b-8` / `PROC-1`），下一批评估。**本轮禁改 OCR 代码**。

## 未做（范围封死）

- 未改 OCR 工具代码；未 commit 代码修复；**未把待确认项写成既成事实**。
- Items：A ✅（本文件） / B ✅（`docs/OCR-FOLLOWUPS-INDEX.md`）。任何第三条插入 → 登记 follow-up，未现场做。
