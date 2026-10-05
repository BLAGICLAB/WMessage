# Batch Spec: G4-G6-AFFINITY-r1 linLog 回退 hotfix

```json
{
  "batch_id": "G4-G6-AFFINITY-r1",
  "family": "task-graph",
  "expected_files": [
    "docs/batches/G4-G6-AFFINITY-r1.spec.md",
    "src/components/GraphPage/graph-adapter.ts"
  ],
  "max_lines_added": 30,
  "max_lines_removed": 10,
  "findings": [
    {"id": "G3-S1", "file": "src/components/GraphPage/graph-adapter.ts", "line": 1, "fix": "回退 linLogMode 实验——LinLog 对无连线孤点无引力约束被斥力炸飞（真机『节点一闪而过』根因）；分扇区初值已保证聚簇"}
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

真机回归 hotfix：老板报告图谱打开后「节点一闪而过，画布就没有节点了」。
根因 = r1 引入的 `linLogMode: true` 实验：LinLog 能量模型对无连线孤点无引力
约束，FA2 起跑后真实数据里的大量孤点（无标签无依赖）被斥力无限推远飞出视野。
1227 压测连通性好未复现。回退为 false——聚簇由分扇区确定性初值保证，
不依赖 linLog。
