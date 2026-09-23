# HOOK-AUDIT-01 — pre-push 门禁完整性事实矛盾（仅调查，不下结论）

> 立项：2026-09-23，TEST-FIX-01（f0da075）修复过程中暴露。
> 性质：**门禁完整性事实矛盾**的验证问题，不是代码修复。本 issue 只调查事实，
> **不改 hook、不改 test-all.sh、不改 push 流程**。验完成立后才谈修法。
> v2（2026-09-23 reviewer 收紧）：§1 表加 session 列 + f0da075 移出矛盾样本；
> §2 修正 shell history 过强推断；§3.1 删未证实主体名；§3 加第 5 条。

## 1. 矛盾陈述（四件不能同真，不解释、不下结论）

1. pre-push hook 由 `aebf005`（2026-08-18 12:10）引入，**早于**测试引入近一个月
2. `.githooks/pre-push` → `scripts/test-all.sh` → **全量 `cargo nextest run`（无过滤）**
3. 测试 `bot_fs::tests::always_allow_uses_user_input_dir_not_canonical_parent` 由
   `3266382`（2026-09-21 12:45）引入；在本机 macOS 上**确定性失败**
   （`/var` firmlink → canonicalize 出 `/private/var`，setup 未规范化，断言恒假；
   3/3 复现 + checkout `71a35bf` 实跑同挂）
4. 但该测试引入后，**7 次 push 全部成功**（reflog 实核，tip 均含 `3266382`）：

   | push 时刻（reflog） | tip | 含 3266382 | session/环境标注 |
   |---|---|---|---|
   | 2026-09-21 21:43:50 | 898e88a | ✓ | 未知 |
   | 2026-09-22 18:41:55 | 87fd99d | ✓ | 未知 |
   | 2026-09-23 11:27:07 | be53182 | ✓ | 未知 |
   | 2026-09-23 12:34:04 | 8e7c9b4 | ✓ | 未知 |
   | 2026-09-23 15:03:55 | 4adbf1e | ✓ | 未知 |
   | 2026-09-23 15:24:44 | adb154c | ✓ | 未知 |
   | 2026-09-23 18:18:33 | 71a35bf | ✓ | 未知 |

   分布：09-21 ×1 / 09-22 ×1 / 09-23 ×5。session/环境列**全部未知**——reflog 不记
   发起主体；此列是后续调查的关键输入，需靠 §3 动作回填。

   **对照（非矛盾样本）**：修复后 `f0da075` 于 2026-09-23 21:56:32 push 成功
   （hook 实跑全绿）——含 `3266382` **且测试已修复**，作为修复后 hook 实跑基线。

## 2. 已核事实（2026-09-23 取证，作为基线）

- `git config --get core.hooksPath`（当前值）= `.githooks`（生效中——21:02 与 21:4x
  两次 push attempt 均被 hook 真实拦截：BotDisabled flaky 一次 + 本测试一次）
- `.git/hooks/pre-push` **不存在** → 无旧 hook 遮蔽可能（此候选已排除）
- shell history（zsh/bash/fish）**无 `git push` 记录**。**这不能证明 push 非交互
  shell 发起**——history 被清 / 非交互 / 其他 shell 工具都是合法解释。
  `--no-verify` 与否从 shell history 路径**无法证实也无法证伪**。
- `git reflog` 只记 `update by push`（tip + 时刻），**不记 push 命令行参数**
  → reflog 路径无法直接回答是否 `--no-verify`
- 本机 git identity 基线（供 §3.5 对照）：`user.name=BLAGICLAB`，
  `user.email=zxf_83117@163.com`，SSH key `~/.ssh/id_ed25519.pub`
  指纹 `SHA256:XwF7t0D1QuJRWHAqx/va6V23Z8/3stxepbZotYbKzyQ`

## 3. 待验证方式（下一步调查动作）

1. 今日三次（12:34 / 15:24 / 18:18）push 的**发起主体确认**：会话 agent /
   IDE 工具 / 人工。**各主体 hook 行为不同**（IDE 推送默认是否跑 hook 需查该
   工具文档——此查询目的是确定"当时是否可能 hook 未跑"，不引入任何未证实主体）
2. 若发起主体可定位到具体工具 → 查该工具是否传 `--no-verify` / 是否设置
   `core.hooksPath` / 是否在本机执行（他机 macOS 版本 /var 解析行为相同，
   但 Linux/Windows 上该测试本就不挂——**他机推送是合法解释之一，需证**）
3. 历史 `core.hooksPath` 值：若上述都不可得，当前值（`.githooks`）作基线，
   记"历史值不可考"
4. 验证完成前，任何"hook 当时没跑 / 跑了但放过"的说法都是假设，**禁止写入
   commit message / 审计链**
5. **本机是否就是当时 push 的执行机。** 对照 §2 基线（git user.name/email +
   SSH key 指纹）+ reflog 时间戳与本机 cron/log 记录——排除"他机 push 但
   tip 同步"的可能。**这条若成立，谜题收敛到"本机 hook 为什么没拦"；
   若不成立，问题迁到"哪台机在推"。**

## 4. 范围声明

- 本 issue **只调查事实**
- **不改** `.githooks/`、`scripts/test-all.sh`、push 流程、batch 纪律
- 修法（若需要）待事实查清后另行立项
