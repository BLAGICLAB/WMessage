# Batch Spec: EVNB-01 (v1)

## 目的

error-visible-non-blocking family 收口：BT-01a 退批登记的 2 条（C5-BT-01a-1 / C5-BT-01a-2），
修法 (b) / (d) 已在退批登记中拍定，与已落批的 C5-AP-06（api_server.rs broadcast eprintln）同 family 同形态。

## 人类可读摘要

- family: error-visible-non-blocking
- 覆盖 findings: 2（C5-BT-01a-1 = commands.rs:30+:31 一个 finding 两站；C5-BT-01a-2 = keyring.rs:275-277）
- 预估 diff: 2 files / +15/-5 lines（A 类行内展开：单点 1→4 行）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5

## 红线

- family 一致性：只含 error-visible-non-blocking（log 可见 + 不阻断），无错误传播改动
- **不改签名、不改调用点**（delete_api_key_at / bot_get_config 签名不变，无 ripple）
- 日志落点：commands.rs 走 bot.log（audit::audit_log，AppHandle 在场）；keyring.rs 无 AppHandle → eprintln（与 C5-AP-06 同先例）
- 错误字符串入日志前过 audit::escape_for_log（防日志注入 / 多行撕裂）

## Stop 条件

- compile_failure / architecture_blocker / family_heterogeneity / new_high_different_root / gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EVNB-01",
  "family": "error-visible-non-blocking",
  "expected_files": [
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/keyring.rs"
  ],
  "max_lines_added": 28,
  "max_lines_removed": 5,
  "findings": [
    {
      "id": "C5-BT-01a-1",
      "file": "src-tauri/src/bot/config/commands.rs",
      "line": 30,
      "fix": "bot_get_config 内 :30/:31 两处 let _ = io::migrate_legacy_key / migrate_search_keys（均 -> Result<(), String>，io.rs:137/:167 实核）。修法 (b) log warn + 继续：if let Err(e) = ... { audit::audit_log(&app, ...) }（audit 已 import :13；escape_for_log 过错误串）。幂等可重试 + 非前置副作用，不阻断读取。无 ripple（命令签名不变）。"
    },
    {
      "id": "C5-BT-01a-2",
      "file": "src-tauri/src/bot/config/keyring.rs",
      "line": 275,
      "fix": "delete_api_key_at System 分支 :275-277 let _ = old.delete_credential()。修法 (d)：保留原顺序（主删除先算 let r = ...），r.is_ok() 时才做遗留清理；清理失败 eprintln! 不阻断（返 r，主目标达成）；主删除失败直接返 Err（遗留未动，无副作用）。无 AppHandle → eprintln（C5-AP-06 先例）。无 ripple（pub(crate) 签名不变）。"
    }
  ],
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

## 签名核（执行前已验）

- `io::migrate_legacy_key(app: &AppHandle) -> Result<(), String>`（io.rs:137）
- `io::migrate_search_keys(app: &AppHandle) -> Result<(), String>`（io.rs:167）
- `delete_api_key_at(backend, file, slot) -> CommandResult<()>`（keyring.rs:263）——System 分支 `let r = ...` 已在场（:270-272），只需给遗留清理加 `r.is_ok()` 守卫 + 错误分流
- `audit::audit_log(app: &AppHandle, line: &str)`（config/audit.rs:22）；`audit::escape_for_log(s: &str, max: usize) -> String`（config/audit.rs:56，pub(crate)）
- commands.rs 已 `use super::audit;`（:13）——无新 import
- 无调用点变化（两函数签名均不动）→ 无 ripple 文件

## 调用点影响清单

| # | file:line | 现状 | 目标 | 签名变？ |
|---|---|---|---|---|
| 1 | commands.rs:30 | `let _ = io::migrate_legacy_key(&app);` | `if let Err(e) = ... { audit_log }` | 否 |
| 1 | commands.rs:31 | `let _ = io::migrate_search_keys(&app);` | 同上 | 否 |
| 2 | keyring.rs:275-277 | `if let Ok(old) = ... { let _ = old.delete_credential(); }` | `if r.is_ok() { if let Ok(old) = ... { if let Err(e) = ... { eprintln! } } }` | 否 |

## Budget 逐点算（SOP §5：A 类行内展开）

| # | 改动点 | 类 | + | - |
|---|---|---|---|---|
| 1 | commands.rs:30-31 两处 let _ → if let Err 块（各 1→4 行） | A | 8 | 2 |
| 2 | keyring.rs:275-277 3 行 → 7 行（r.is_ok() 守卫 + 嵌套错误分流） | A | 7 | 3 |
| **合计** | | | **15** | **5** |

**budget: max_lines_added: 28 / max_lines_removed: 5**（初估 +15/-5，实测 +28/-5——
A 类公式未计 rustfmt 多行展开开销：嵌套调用 `audit_log(&app, &format!(...))` 超行宽被 fmt
拆成 8 行。估算错一次校正，scope 未变（同 2 findings / 同 2 文件 / 同形态）。
公式教训：含嵌套调用的行内展开应按 fmt 后行数估，不按逻辑行数估。）

## 提交信息骨架

```
fix(bot): EVNB-01 — error-visible-non-blocking 收口（BT-01a 退批 2 条）

【family】error-visible-non-blocking（错误日志可见 + 不阻断调用方；与 C5-AP-06 同形态）
【实修 2 条 / 3 站】
- commands.rs:30/:31 — bot_get_config 两处 migrate let _ → if let Err + audit_log（bot.log）
  修法 (b)：幂等可重试 + 非前置副作用，log warn 后继续
- keyring.rs:275-277 — delete_api_key_at v0 遗留清理 let _ → r.is_ok() 守卫 + 双 Error 分流
  修法 (d)：主删除失败返 Err（遗留未动）；主删除成功 + 清理失败 eprintln 不阻断
【D2】行为断言：migrate/清理失败时有 bot.log/eprintln 可见痕迹且返回值不变；
前置断言：主删除成功才触发遗留清理；反例断言：若主删除失败，遗留条目必须原样保留
（否则「清除失败但旧 key 被清」= 半清状态）。
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/EVNB-01.spec.md python3 scripts/batch-verify.py docs/batches/EVNB-01.spec.md
```

## 不在本批

- error-not-propagated family 剩余（C5-EV-3b-A 退 triage 重做；首批余量）
- failure-recovery-default-value（C5-DB-05 / C5-MI-03）
- 新增测试：keyring System 分支真实存储测试环境不可用（keyring.rs:292 注释明记）；commands.rs 需 AppHandle mock——与 C5-AP-06 同处置（无新测试，靠三层 + OCR）
