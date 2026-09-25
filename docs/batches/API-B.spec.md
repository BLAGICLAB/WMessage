# Batch Spec: API-B

## 目的

落地用户拍板的 B 类决策 2 项（api 域，2026-09-25 拍板）：#16 SSE 落盘写失败拒推进 id（A）、#15 flag 锁外 TOCTOU 残余窗文档化（C）。对应 PHASE2-TRIAGE §4 B 类权威清单 #15/#16。

## 人类可读摘要

- family: api-bclass-failclosed
- 覆盖: B 类 2 项（1 实修 + 1 注释文档化）
- 预估 diff: 2 文件 / +45/-3（无新文件）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 3

## 逐条处置（原文均已核 + 用户拍板 2026-09-25）

**实修 ① src-tauri/src/api_server.rs:153（C5-AP-06 残留，用户拍 #16=A）**：broadcast 内 `atomic_write(id)` 失败仅 eprintln，id 已 fetch_add 推进且事件照常广播——重启后 Last-Event-ID 与盘面脱节，客户端按 id 去重会静默丢事件段。
**修法**：写失败 → `fetch_sub` 归还 id + 事件不进 history、不推送（fail-closed，「要么持久化要么不推进」）；eprintln 留痕。归还后下一次 broadcast 重新 fetch_add 取同一 id 重试写盘（进程内单临界区，无竞争）；事件本身丢弃（调用方无重试协议）为拍板 A 明示接受。恢复路径：写盘恢复后广播自然接续。
**ripple**：broadcast 签名不变（调用点不动）；id_path=None 的 hub（内存态）不受影响。

**实修 ② src-tauri/src/api_handlers/commands.rs:61 + :238（C5-AP-01 残余，用户拍 #15=C）**：enabled flag 写/清在锁外 + service_present 复核，复核与写/清之间仍有理论 TOCTOU 微窗（并 api_start/api_stop 交错可致 flag 与内存态背离一次）。
**修法**：维持现状 + 注释补全残余窗声明（「复核≠原子：service_present 与 write/clear 之间仍有微窗，后果=下次启动自动恢复状态错一次，用户手动开关即自愈；锁内 I/O 违反『文件 I/O 不持锁』既有约定，世代号 CAS 方案成本高——均未取（拍板 C）」）。零行为变更。
**ripple**：无。

## 测试（api_server.rs 既有 #[cfg(test)] 内新增，直调 EventHub）

1. `broadcast_persist_failure_fails_closed_and_retries_same_id`：persisted 指向不可写路径（路径为目录 → atomic_write 必败）→ broadcast 后 last_id 不变（归还被拒的 id）+ history 空 + client 队列无事件；换可写路径重建 hub（或同 hub 无法换 path——新 hub）验证后续正常广播 id 从 1 重新接续
2. 既有 `broadcast_concurrent_delivery_order_matches_id_order` 不回归（test-all 担保）

## 红线

- family 一致性：只含 api 域 2 项 B 类拍板落地
- broadcast 单临界区结构不动（AP-04 已定）；fetch_sub 必须在 clients 锁内（与 fetch_add 同临界区）
- 不动 vendor / sse.rs writer 侧
- 新注释不引用审计批次号

## spec 起草后自查三条

1. expected_files：src-tauri/src/api_server.rs + src-tauri/src/api_handlers/commands.rs = 2（全路径已列）
2. budget：A 类（fail-closed 块 +10/-1；注释 +8）+ B 类（测试 ≈ +28）合计 ≈ +46/-3，上限 +80/-15；无新文件
3. fix 字段 ripple：① broadcast 内闭环签名不变；② 纯注释

## 自主执行规则

spec 经用户拍板（B 类 2 项方向 2026-09-25）后：
- agent 全权执行至 commit，不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + push 两值
- 过程证据落 ~/.openclaw/cache/API-B/，不喊人

## Stop 条件（触发即停，报用户）

- compile_failure（无 X 形态退路）
- architecture_blocker（签名/调用点超预算）
- family_heterogeneity
- new_high_different_root（OCR r1 出的 high 非本批根因）
- gate_fail（batch-verify FAIL 且非 spec 声明调整可解）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "API-B",
  "family": "api-bclass-failclosed",
  "expected_files": [
    "src-tauri/src/api_server.rs",
    "src-tauri/src/api_handlers/commands.rs"
  ],
  "max_lines_added": 80,
  "max_lines_removed": 15,
  "findings": [
    {"id": "C5-AP-06.2", "file": "src-tauri/src/api_server.rs", "line": 153, "fix": "broadcast 落盘写失败 → fetch_sub 归还 id + 事件不进 history 不推送（fail-closed 拍板 A）+ eprintln；下次广播重取同 id 重试。ripple：无（签名不变）"},
    {"id": "C5-AP-01.3", "file": "src-tauri/src/api_handlers/commands.rs", "line": 61, "fix": "flag 写/清残余 TOCTOU 微窗注释声明（复核≠原子、后果可自愈、锁内 I/O 与 CAS 均未取=拍板 C）。:238 同型。ripple：无（纯注释）"}
  ],
  "assertions_min": {
    "src-tauri/src/api_server.rs": 4
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
  },
  "stop_conditions": [
    "compile_failure",
    "architecture_blocker",
    "family_heterogeneity",
    "new_high_different_root",
    "gate_fail"
  ]
}
```

## 提交信息骨架

```
fix(api): API-B — api-bclass-failclosed（B 类拍板落地 2 项）

【family】SSE 落盘 fail-closed / flag 残窗文档化（用户拍板 2026-09-25：#16=A、#15=C）
【实修 2 处】
- api_server.rs:153 broadcast 落盘写失败 → 归还 id + 事件不投递（要么持久化要么不推进；下次广播重取同 id 重试）
- api_handlers/commands.rs:61/:238 flag 残余 TOCTOU 微窗注释声明（维持现状，可自愈）
【行为变更】落盘失败时 SSE 事件丢弃不推进 id（原为照常广播致重启后 Last-Event-ID 脱节）
【测试】+1（persist 失败 fail-closed + 同 id 重试接续）
【OCR】r1：<N> comments <处置>
【基线】D2: files=2(+A/-R) asserts=… tests=…
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/API-B.spec.md python3 scripts/batch-verify.py docs/batches/API-B.spec.md
```
