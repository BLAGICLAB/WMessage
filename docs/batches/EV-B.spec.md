# Batch Spec: EV-B

## 目的

落地用户拍板的 B 类决策（evolution 域，2026-09-25 拍板）：#17 read_all 损坏行折中语义（C：首行坏 → Err 结构级损坏；中间坏行 → 跳过 + stderr 留痕）。#23（ProposalTarget camelCase）侦察发现拍板前提疑似不成立（TS/消费方已全 snake_case 一致），独立 reviewer 核验中——核验结论另登记，不入本批代码。对应 PHASE2-TRIAGE §4 B 类权威清单 #17。

## 人类可读摘要

- family: evolution-jsonl-corrupt-tolerance
- 覆盖: B 类 1 项（1 实修；#23 视 reviewer 结论零代码或另批）
- 预估 diff: 4 文件 / +120/-8（无新文件）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 4

## 逐条处置（原文均已核 + 用户拍板 2026-09-25）

**实修 ① src-tauri/src/evolution/change/record.rs:201 + candidate/entry.rs:103（C5-EV-3b-A 残余，用户拍 #17=C）**：两处 read_all 逐行解析 jsonl，单行 JSON 损坏 → 整读 Err——生产调用方 load_changes/load_proposals 报错 = evolution 面板整挂；candidate/mod.rs:52 write_proposals 更以 unwrap_or_default() 吞 Err（dedup 基线静默丢失 → 重复提案再追加）。
**修法**：两处 read_all 统一改「首行（结构级）坏 → Err；中间坏行 → eprintln 留痕（行号+错误）+ 跳过，返回好行」——折中：文件头损坏不可恢复即 Err（fail-closed），中间单行损坏不拖死全部读取（韧性）。留痕用 eprintln（read_all 为纯函数无 AppHandle，同 evolution_activation 先例）。同根对齐：candidate/mod.rs:52 `unwrap_or_default()` → `?` 传播（dedup 基线不可得时不得盲写——Err 语义恢复后不再吞）。
**ripple**：candidate/mod.rs write_proposals 一行（Err 传播）；panel/commands.rs load_* 签名不变（Err 上游已有处理）；中间坏行不再 Err = 行为变更（面板对部分损坏文件恢复可用）。

## 测试（record.rs / entry.rs 各 2，直调 read_all）

1. 中间行损坏（好/坏/好）→ Ok 两条好行
2. 首行损坏 → Err
3. candidate/mod.rs：write_proposals 对 read_all Err 的传播由签名担保（既有调用链编译担保，行为属 1/2 测试的推论）

## 红线

- family 一致性：只含 evolution 域 #17 拍板落地
- 不改 append / write_proposals RMW 语义（除 unwrap_or_default → ? 一行）
- 两处 read_all 行为必须一致（同族同语义）
- 新注释不引用审计批次号

## spec 起草后自查三条

1. expected_files：evolution/change/record.rs + evolution/candidate/entry.rs + evolution/candidate/mod.rs = 3（全路径已列）
2. budget：A 类（两处 read_all 循环改造 ≈ +30/-8；mod.rs 一行 ≈ +1/-1）+ B 类（4 测试 ≈ +80）合计 ≈ +115/-8，上限 +170/-20；无新文件
3. fix 字段 ripple：mod.rs 一行已入 expected_files；panel/commands.rs 签名不变零改动

## 自主执行规则

spec 经用户拍板（B 类方向 2026-09-25）后：
- agent 全权执行至 commit，不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报：hash + 批定性 + OCR 轮次 + comments 处置 + push 两值
- 过程证据落 ~/.openclaw/cache/EV-B/，不喊人

## Stop 条件（触发即停，报用户）

- compile_failure（无 X 形态退路）
- architecture_blocker（签名/调用点超预算）
- family_heterogeneity
- new_high_different_root（OCR r1 出的 high 非本批根因）
- gate_fail（batch-verify FAIL 且非 spec 声明调整可解）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-B",
  "family": "evolution-jsonl-corrupt-tolerance",
  "expected_files": [
    "src-tauri/src/evolution/change/record.rs",
    "src-tauri/src/evolution/candidate/entry.rs",
    "src-tauri/src/evolution/candidate/mod.rs"
  ],
  "max_lines_added": 170,
  "max_lines_removed": 20,
  "findings": [
    {"id": "C5-EV-3b-A.17", "file": "src-tauri/src/evolution/change/record.rs", "line": 201, "fix": "read_all 折中语义（拍板 C）：首行坏 → Err（结构级损坏 fail-closed）；中间坏行 → eprintln 留痕 + 跳过返回好行。candidate/entry.rs 同形改造。ripple：candidate/mod.rs:52 unwrap_or_default → ?（Err 传播恢复，dedup 基线不盲写）"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/change/record.rs": 4,
    "src-tauri/src/evolution/candidate/entry.rs": 4
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
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
fix(evolution): EV-B — evolution-jsonl-corrupt-tolerance（B 类拍板落地 #17=C）

【family】jsonl 损坏行折中容忍
【实修 1 处（三文件）】
- record.rs/entry.rs read_all：首行坏 → Err（结构级损坏）；中间坏行 → 留痕跳过返回好行
- candidate/mod.rs write_proposals unwrap_or_default → ?（Err 语义恢复后 dedup 基线不再盲吞）
【行为变更】部分损坏的 jsonl 不再整挂 evolution 面板；损坏文件上的写路径不再以空基线盲写
【测试】+4（中段坏行跳过 / 首行坏 Err ×2 文件）
【OCR】r1：<N> comments <处置>
【基线】D2: files=3(+A/-R) asserts=… tests=…
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/EV-B.spec.md python3 scripts/batch-verify.py docs/batches/EV-B.spec.md
```
