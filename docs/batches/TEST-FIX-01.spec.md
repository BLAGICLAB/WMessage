# Batch Spec: TEST-FIX-01 (v2)

## 目的

主干测试修复（**非 family 批**）。`bot_fs::tests::always_allow_uses_user_input_dir_not_canonical_parent`（src-tauri/src/bot_fs.rs:1035-1078）在 macOS 上**确定性失败**（3/3 复现 + 71a35bf 同样挂）：

- `std::fs::canonicalize` 把 `/var`（firmlink）解析为 `/private/var`
- setup 中 `outside` 由 `tmp.path().join("outside")` 拼出、**未规范化**
- 第三个 sanity 断言 `old_dir == outside` 恒假（left=/private/var/…，right=/var/…）

**修法：setup 时规范化 `outside`**（不是逐条断言 canonicalize）——先决条件是"outside 是 canonical 视角的 outside"，断言原样成立。

```rust
// create_dir_all(&outside) 之后插一行 shadow（+1 注释 +1 代码，共 +2/-0）：
// macOS firmlink：/var → /private/var，setup 即规范化，断言在 canonical 视角比较
let outside = std::fs::canonicalize(&outside).unwrap();
```

## 人类可读摘要

- family: 无（主干测试修复，非 family finding）
- 覆盖 findings: 1
- 预估 diff: 1 file / +2/-0 lines
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5（**不豁免**；若报"断言削弱"→ 按 FP 登记：断言数 44 不变、语义不变，仅 setup 比较基准换 canonical）

## 红线

- **只动测试 setup，不动 SUT**（resolve_with_perm / allowlist 生产代码零改动）
- **不断言削弱**：assert 宏总数 44 → 44，三条断言语义全保留（new_dir == sub / old_dir != sub / old_dir == outside）
- **不扩散**：bot_fs.rs:972 / :1010 另两处 `let outside = tmp.path()…` 所在测试目前全绿，不在本批

## Stop 条件

- compile_failure / gate_fail / scope_creep（同套件其他测试转红）/ new_high_different_root（OCR 报出与本修复无关的 high）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "TEST-FIX-01",
  "family": "trunk-test-fix",
  "expected_files": [
    "src-tauri/src/bot_fs.rs"
  ],
  "max_lines_added": 2,
  "max_lines_removed": 0,
  "assertions_min": {
    "src-tauri/src/bot_fs.rs": 44
  },
  "findings": [
    {
      "id": "TEST-FIX-01-1",
      "file": "src-tauri/src/bot_fs.rs",
      "line": 1040,
      "fix": "always_allow_uses_user_input_dir_not_canonical_parent（:1035-1078）setup 规范化：let outside = tmp.path().join(\"outside\") 保持，create_dir_all(&outside) 之后加 shadow let outside = std::fs::canonicalize(&outside).unwrap()（+1 行注释说明 firmlink）。此后 symlink target / std::fs::write 均走 canonical 路径，canonicalize(user_input) 的 parent 与 outside 同视角，第三断言 old_dir == outside 在 macOS 成立。断言数 44 不变，语义不变。"
    }
  ],
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 5
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail",
    "scope_creep",
    "new_high_different_root"
  ]
}
```

## 签名核（执行前已验）

- 失败断言现场（pre-push hook 输出 + 单测 3/3 复现）：left=`/private/var/folders/…/outside`，right=`/var/folders/…/outside`
- `71a35bf`（origin/main）checkout 实跑：**同样挂**（同断言同 firmlink 差）→ 主干坏确认
- 修后断言推演：old_dir = canonicalize(link).parent() = canonical(outside) == outside ✓；old_dir != sub（前缀 /private/var vs /var，且路径不同）✓；new_dir = expanded.parent() = sub（不受影响）✓
- bot_fs.rs 当前 assert 宏总数 = 44，实测命令与输出：

  ```
  $ grep -cE '\b(assert|assert_eq|assert_ne|debug_assert|debug_assert_eq|debug_assert_ne)!' src-tauri/src/bot_fs.rs
  44
  ```

  （与 batch-verify.py `count_asserts_staged` 同一正则族，staged 后应为 44）

## Budget 逐点算

| # | 改动点 | 类 | + | - |
|---|---|---|---|---|
| 1 | bot_fs.rs:1042 后插 shadow 行 + 1 行注释 | A | 2 | 0 |
| **合计** | | | **2** | **0** |

**budget: max_lines_added: 2 / max_lines_removed: 0**（预估 +2/-0；若实际 numstat 出 -行则按 SOP §5 校正一次）

## 提交信息骨架

```
test(bot_fs): TEST-FIX-01 — always_allow 测试 setup 规范化 outside（macOS firmlink 修复）

【性质】主干测试修复（非 family 批）。bot_fs.rs:1074 第三断言在 macOS 确定性失败：
canonicalize 把 /var firmlink 解析为 /private/var，setup 的 outside 未规范化，恒假。
【修法】setup 即规范化：create_dir_all 后 shadow let outside = std::fs::canonicalize(&outside)
（+2/-0）。断言 44→44 不变，三条断言语义全保留；SUT 零改动。
【史】测试由 3266382(2026-09-21) 引入并已在 origin/main；pre-push hook 由
aebf005(2026-08-18) 引入，早于测试；本机 checkout 71a35bf 实跑同挂（确定性）。
今日 12:29 / 15:23 / 18:18 三次成功 push 与全量门禁的矛盾不在本批下结论，
单开 issue 追查（PHASE2-TRIAGE §3.5 登记为 follow-up）。
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/TEST-FIX-01.spec.md python3 scripts/batch-verify.py docs/batches/TEST-FIX-01.spec.md
```

## 背景：①②③ 核查结论（2026-09-23）

1. **测试史**：`3266382`（2026-09-21）引入，已在 origin/main；其后 `8b0dfdb`、`ffb79ff`（均动 bot_fs.rs）也在 origin/main —— 引入后主干推进过多次
2. **71a35bf 实跑**：挂（同断言）→ 真实主干坏，非本批引入
3. **hook 原文**：`.githooks/pre-push` → `scripts/test-all.sh` → **全量 `cargo nextest run`（无过滤）** + pytest + vitest；hook 由 `aebf005`（2026-08-18）引入，**早于**测试一个月
4. **未解矛盾（单开 issue，不在本批下结论）**：hook 全量无过滤 + 测试确定性挂 + 今日三次成功 push（origin/main commit 时间簇 12:29 / 15:23 / 18:18）三者不能同真

## 不在本批

- **hook bypass 矛盾追查**：单开 issue（PHASE2-TRIAGE §3.5 登记），需查 09-21 以来各次 push 的实际执行环境
- **bot_fs.rs:972 / :1010 同形 `let outside`**：所在测试全绿，不动
