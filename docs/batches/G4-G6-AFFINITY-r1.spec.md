# Batch Spec: G4-G6-r1 分布公式修正（孤点窄环带挤死 → 面密度均匀圆盘）

```json
{
  "batch_id": "G4-G6-AFFINITY-r1",
  "family": "task-graph",
  "expected_files": [
    "docs/batches/G4-G6-AFFINITY-r1.spec.md",
    "src/components/GraphPage/graph-adapter.ts"
  ],
  "max_lines_added": 30,
  "max_lines_removed": 20,
  "findings": [
    {"id": "G3-S1", "file": "src/components/GraphPage/graph-adapter.ts", "line": 1, "fix": "无组节点分布公式：固定 0.68~1.0 窄环带（%400 循环）→ 面密度均匀圆盘（r ∝ √(i/n)，黄金角散角度）——修小任务量节点挤死一圈（方案四诊断）"}
  ],
  "assertions_min": {
    "src/components/GraphPage/graph-adapter.ts": 0
  },
  "ocr_plan": {
    "rounds": 0,
    "timeout_seconds": 600,
    "expected_max_comments": 1
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

方案四（老板诊断）落地：30 个任务的图，节点全挤在半径 73~78px 的一圈细环带
里——旧公式 `r = GROUP_RING × (0.68 + 0.32×√((freeIdx%400+1)/400))` 是固定
窄环带且 %400 循环，与节点数量无关。修为面密度均匀圆盘：`r ∝ √((i+0.6)/n)`
铺满半径、黄金角散角度；半径分位实测 p10=382 → p25=563 → p50=762 → p75=891
→ p90=952 单调铺满，无环带聚集。
