# Batch Spec: U0-HANDOFF

## 目的

UI 改造战役立项入库（纯文档，零代码）：

1. `docs/UI-REDESIGN-PROPOSAL-2026-09-30.md`——已拍板方案补交入库（上会话产出，
   四项拍板 + 截图拆解 + 分期方案）。
2. `docs/UI-REDESIGN-HANDOFF-2026-09-30.md`——新会话交接文档：批次定义 U1–U4、
   UI 红线、spec/门禁速查、基线快照、开工提示词。
3. `DEVLOG.md` 顶部加 UI 战役立项条目。

## 红线

- 纯文档批：零源码/配置改动，不碰 src、src-tauri、scripts。
- 文档内容与既有事实一致：基线 commit、文件行数、门禁机制均按交接日实测填写。

## 测试

- 无代码路径：test-fast 智能跳过；pre-commit 钩子对本批不触发 batch-verify
  （staged 不含 src/src-tauri），test-fast 照跑。

## spec 起草后自查三条

1. expected_files 即本次 staged 全集（3 项，含本 spec 文件自身）。
2. budget：DEVLOG +14/-0；两份新文档 195+105=300 行（new files 预算 350）。
3. findings 留空（纯文档批，无 ocr 代码复审对象；文档人工双查过）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U0-HANDOFF",
  "family": "ui-redesign",
  "expected_files": [
    "DEVLOG.md",
    "docs/UI-REDESIGN-HANDOFF-2026-09-30.md",
    "docs/UI-REDESIGN-PROPOSAL-2026-09-30.md",
    "docs/batches/U0-HANDOFF.spec.md"
  ],
  "max_lines_added": 40,
  "max_lines_removed": 2,
  "max_new_files_lines": 350,
  "findings": [],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
bash scripts/test-fast.sh   # 无 src 改动 → nothing to test / 智能跳过
```
