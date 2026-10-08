# TESTS-AUDIT 月度基线（⑨ 月度例行核对锚点）

> 用途：每月 1 号自动化核对（cron `automation-bbbd0371`）以本文件基线行为对比基准。
> 动机：strict 零 xfail 门禁在 pre-push 生效后，只剩「绕过门禁」（--no-verify / 他机推送）
> 一条回流路径——月度 diff 就是兜住这条路径的探测网（与 HOOK-AUDIT-01 调查动机互补）。

## 基线（2026-09-26 建档，DEC-1 批）

命令：

```bash
python3 -m pytest tests-audit/audit_pre_step_pre_execute.py tests-audit/audit_tauri_bridge.py tests-audit/audit_error_codes.py tests-audit/audit_module_map.py -v
```

汇总行（基线）：

```
38 passed, 1 skipped, 1 warning in 6.57s
```

| 计数项 | 基线 |
|---|---|
| passed | 38 |
| skipped | 1 |
| xfailed | 0（Phase 6 政策：strict 零 xfail 门禁） |
| xpassed | 0 |
| failed | 0 |

## 判定规则

- 任一计数项漂移（含 skipped 变化）或 pytest 退出码非 0 → 报警。
- 报警落点（硬约束「有人能看到」）：`docs/TESTS-AUDIT-MONTHLY-<YYYY-MM-DD>.md`
  （进 repo 可追溯）+ `git push` + macOS 系统通知（osascript）。**只写本地日志不算报警。**
- 无漂移：仅记 `~/.openclaw/cache/wmessage-tests-audit-monthly.log` 一行，不动 repo。

## 基线更新流程

漂移若属有意变更（新增/删除/转正测试且已走批循环），人工核对后用普通 docs commit
更新本文件的汇总行与表格，并在行尾注明批准批名与日期。
