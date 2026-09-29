# Batch Spec: LEDGER-1

## 目的

审计修复战役收尾留账（§10 终验最后一项）：DEVLOG 追记 Mimosa 终扫结论——
seal `sha256:0561d129…96fc`，1166 packages，0 advisory 命中；7 findings 全
medium 且均为误报类（eval dev 工具本地数据源 ×6 + rustdoc 构建产物 ×1），
对照 §1.2 攻击面无回归。纯文档追加。

## 修法

DEVLOG 总账条目的 Mimosa 段由「运行中」改为终验结论。

## 红线

- 纯文档，无代码。

## 测试

- 无代码路径：test-fast 智能跳过。

## spec 起草后自查三条

1. expected_files 2。
2. budget：修改 +14/-4。
3. 无 findings。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "LEDGER-1",
  "family": "repo-hygiene",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/LEDGER-1.spec.md"
  ],
  "max_lines_added": 20,
  "max_lines_removed": 5,
  "findings": [],
  "stop_conditions": ["gate_fail"]
}
```

## 验证命令

```
bash scripts/test-fast.sh
```
