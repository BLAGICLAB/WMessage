# Batch Spec: BOT-B

## 目的

落地用户拍板的 B 类决策 5 项（bot 域为主，2026-09-25 拍板）：#10 剥 key 读回验证（C）、#11 按会话清理（A）、#12 Jina 回退 stderr 留痕（B）、#6 in_quotes wontfix 确认（A：注释+回归测试）、#13 Windows 权限 wontfix（B：文档化）。对应 PHASE2-TRIAGE §4 B 类权威清单 #6/#10/#11/#12/#13。

## 人类可读摘要

- family: bot-bclass-mixed（批内 5 处独立改动，修复设施不共享——均为拍板落地，如实声明）
- 覆盖: B 类 5 项（4 实修 + 1 wontfix 注释固化；#6 含 2 回归测试）
- 预估 diff: 7 文件 / +165/-18（无新文件）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5

## 逐条处置（原文均已核 + 用户拍板 2026-09-25）

**实修 ① src-tauri/src/bot/config/io.rs:161（C5-BT-03 残余，用户拍 #10=C）**：add_allowed_dir 是唯一不走剥 key 内核的 RMW 写路径（write_bot_config_file_locked 剥三 key，add_allowed_dir 直写 write_config_atomic 保留明文）；r5 medium 曾建议统一走剥 key 内核，但 keyring 不可用时明文是唯一副本，剥除=密钥丢失。
**修法**：add_allowed_dir 写盘前对三个 key 槽位独立执行「keyring 写入（仅空槽，不覆盖新值，同 migrate_legacy_key 策略）→ 读回验证」，验证通过才剥该槽明文；任一环节失败保留明文下次再试（零丢失优先）。纯决策内核 `plaintext_strippable(backed: Option<&str>) -> bool` 抽出供单测。剥除发生时 audit_event! Info 留痕。
**ripple**：仅 add_allowed_dir 内部；write_bot_config_file / update_config_file 已剥不动；keyring::has_key_of_slot / read_key_of_slot / write_key_of_slot 既有 API 直用。

**实修 ② src-tauri/src/bot_skills/state.rs:164（C5-BT-07 残余，用户拍 #11=A）**：clear_terminal_skill_runs 全局清理所有会话的终态 run——会话 B 刚终态待 DSL 调度器感知（/stop、工具失败）的 run 会被会话 A 的入口清理误删。
**修法**：签名加 `session_id: Option<&str>`，retain 只清「本会话的终态 run」，其他会话一律不动（与 active_skill_run_for 的 session 匹配语义一致）。调用点 3 处：bot_model_loop.rs:448 传 stop.session_id()；scheduler.rs:283 传自身 session_id 参数；state.rs 既有测试翻新（断言加跨会话隔离：其他会话终态 run 存活）。
**ripple**：SKILL_RUNS 增长注记写入 doc——长期不活跃会话的末条终态记录留存（每会话至多一条，量级可控）；bot_skills mod re-export 签名随动（crate 内部，无外部消费者）。

**实修 ③ src-tauri/src/bot_web.rs:920（C5-BT-05 残余，用户拍 #12=B）**：fetch_text 正文过短回退 Jina Reader 全程静默（`if let Ok` 吞失败、采用/不采用不留痕）。
**修法**：不改 fetch_text 签名（拍板 A 方案扩 scope 不取），回退块三路 eprintln：失败（含错误）/ 采用 / 未采用，均带目标 URL——stderr 留痕承担基本审计（拍板 B 口径）。
**ripple**：无（纯 eprintln，行为不变）。

**实修 ④ src-tauri/src/bot_skills/vars.rs:76（C5-BT-08 残余，用户拍 #6=A 确认 wontfix）**：in_quotes 字节级启发式对常见 JSON 形态（占位符所在字符串内含转义引号等）误判。
**修法**：wontfix 确认落地——replace_ctx 的 in_quotes 处补设计边界注释（启发式是模板构造上非合法 JSON 的结构必然，替换后 serde_json 兜底；若未来误判导致安全/权限错误再转实修）+ 2 回归测试锁当前行为：①占位符紧邻 JSON 字符串内（前后 `"`）→ 替换值 JSON 转义、结果可 parse；②字符串内含转义引号后接占位符（前一字节非 `"`）→ 按现状原样插入（文档化已知限制）。
**ripple**：无（注释+测试）。

**实修 ⑤ src-tauri/src/bot/config/keyring.rs:163（C5-BT-08 残余，用户拍 #13=B wontfix-with-rationale）**：write_key_file_to 降级文件存储权限仅 unix 0600，Windows 无 DACL 等价。
**修法**：wontfix 文档化——doc 注释补「Windows 降级存储无 ACL 收紧，本仓无 Windows 验证手段，权限语义仅在 unix 承诺」；零行为变更。
**ripple**：无。

## 测试

1. io.rs 新增 test mod：plaintext_strippable 三态（Some 有值/空白/None）
2. state.rs 既有 clear_terminal_removes_only_terminal_states 翻新：本会话 Failed 清 / Paused 留 / 其他会话 Failed 留（新隔离保证）
3. vars.rs +2：in_quotes 正常转义路径（结果可 serde_json parse）+ 已知限制路径（现状原样插入锁死）
4. bot_web.rs / keyring.rs：纯 eprintln/注释，无单测挂点——spec 声明

## 红线

- family 如实声明：批内 5 处独立改动，修复设施不共享
- 不动 fetch_text 签名 / 不动 13 个 mutating adapter（#12=#14 的 A 方案均扩 scope，拍板未取）
- 剥 key 失败路径必须保留明文（零丢失优先，拍板 C 核心约束）
- 新注释不引用审计批次号

## spec 起草后自查三条

1. expected_files：bot/config/io.rs + bot_skills/state.rs + bot_model_loop.rs + bot_skills/scheduler.rs + bot_web.rs + bot_skills/vars.rs + bot/config/keyring.rs = 7（全路径已列）
2. budget：A 类（签名/retain/一行调用 ≈ +30/-14）+ B 类（io.rs helper+循环+测试 ≈ +75；vars 测试 ≈ +35；web eprintln ≈ +12；注释 ≈ +12）合计 ≈ +165/-18，上限 +230/-35；无新文件
3. fix 字段 ripple：② 三调用点全列（bot_model_loop/scheduler/state 测试）；① 既有 keyring API 直用零新增导出（knip 不涉 Rust）；④⑤ 纯注释+测试

## 自主执行规则

spec 经用户拍板（B 类 5 项方向 2026-09-25）后：
- agent 全权执行至 commit，不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + push 两值
- 过程证据落 ~/.openclaw/cache/BOT-B/，不喊人

## Stop 条件（触发即停，报用户）

- compile_failure（无 X 形态退路）
- architecture_blocker（签名/调用点超预算）
- family_heterogeneity
- new_high_different_root（OCR r1 出的 high 非本批根因）
- gate_fail（batch-verify FAIL 且非 spec 声明调整可解）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "BOT-B",
  "family": "bot-bclass-mixed",
  "expected_files": [
    "src-tauri/src/bot/config/io.rs",
    "src-tauri/src/bot_skills/state.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_skills/scheduler.rs",
    "src-tauri/src/bot_web.rs",
    "src-tauri/src/bot_skills/vars.rs",
    "src-tauri/src/bot/config/keyring.rs"
  ],
  "max_lines_added": 230,
  "max_lines_removed": 35,
  "findings": [
    {"id": "C5-BT-03-r5", "file": "src-tauri/src/bot/config/io.rs", "line": 161, "fix": "add_allowed_dir 写盘前三 key 槽位 keyring 写入+读回验证，验证过才剥明文（失败保留零丢失）；纯决策内核 plaintext_strippable + 单测；audit_event Info 留痕。ripple：无（既有 keyring API 直用）"},
    {"id": "C5-BT-07.3", "file": "src-tauri/src/bot_skills/state.rs", "line": 164, "fix": "clear_terminal_skill_runs 加 session_id 参数只清本会话终态 run（跨会话隔离）；调用点 bot_model_loop.rs:448 / scheduler.rs:283 / state.rs 测试翻新。ripple：三调用点"},
    {"id": "C5-BT-05.4", "file": "src-tauri/src/bot_web.rs", "line": 920, "fix": "Jina 回退三路 eprintln（失败/采用/未采用，带 URL）——不改 fetch_text 签名。ripple：无"},
    {"id": "C5-BT-08.5", "file": "src-tauri/src/bot_skills/vars.rs", "line": 76, "fix": "wontfix 确认：in_quotes 设计边界注释 + 2 回归测试锁当前行为（转义路径可 parse / 已知限制路径原样插入）。ripple：无"},
    {"id": "C5-BT-08.4", "file": "src-tauri/src/bot/config/keyring.rs", "line": 163, "fix": "wontfix-with-rationale 文档化：Windows 降级存储无 ACL 收紧，权限语义仅 unix 承诺。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/config/io.rs": 3,
    "src-tauri/src/bot_skills/state.rs": 8,
    "src-tauri/src/bot_skills/vars.rs": 6
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 5
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
fix(bot): BOT-B — bot-bclass-mixed（B 类拍板落地 5 项）

【family】批内 5 处独立改动，修复设施不共享（拍板落地批，如实声明）
【实修 5 处】
- bot/config/io.rs add_allowed_dir 剥 key 前置 keyring 读回验证（失败保留明文）
- bot_skills/state.rs 终态清理按会话隔离（三调用点随动）
- bot_web.rs Jina 回退三路 stderr 留痕（签名不动）
- bot_skills/vars.rs in_quotes wontfix 注释 + 2 回归测试
- bot/config/keyring.rs Windows 权限 wontfix 文档化
【行为变更】keyring 读回不过时文件明文不再被剥（原方案有丢钥风险）；他会话终态 run 不再被入口清理误删
【测试】+5（io 三态 / vars ×2 / state 翻新加隔离断言）
【OCR】r1：<N> comments <处置>
【基线】D2: files=7(+A/-R) asserts=… tests=…
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/BOT-B.spec.md python3 scripts/batch-verify.py docs/batches/BOT-B.spec.md
```
