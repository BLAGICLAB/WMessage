# Batch Spec: G3-SIGMA-r1 按成员着色图例与节点同色

```json
{
  "batch_id": "G3-SIGMA-r1",
  "family": "task-graph",
  "expected_files": [
    "docs/batches/G3-SIGMA-r1.spec.md",
    "src/components/GraphPage/GraphCanvas.tsx",
    "src/components/GraphPage/GraphPage.tsx",
    "src/components/GraphPage/graph-adapter.ts",
    "src/components/GraphPage/graph-adapter.test.ts",
    "DEVLOG.md"
  ],
  "max_lines_added": 260,
  "max_lines_removed": 120,
  "findings": [
    {"id": "G3-S1", "file": "src/components/GraphPage/graph-adapter.ts", "line": 1, "fix": "着色双轨键（ownerKey/statusKey 建图双写，切模式免重建）+ ownerOrder 注入 + resolveOwnerColor 导出（chips 与节点共用）"},
    {"id": "G3-S2", "file": "src/components/GraphPage/GraphCanvas.tsx", "line": 1, "fix": "reducer 按模式现场选键；ownerOrder 改 props 注入"},
    {"id": "G3-S3", "file": "src/components/GraphPage/GraphPage.tsx", "line": 1, "fix": "ownerOrder 单一事实源（chips 全序）+ chipColor() 同一解析"}
  ],
  "assertions_min": {
    "src/components/GraphPage/graph-adapter.test.ts": 0
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1200,
    "expected_max_comments": 3
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

修老板验收反馈：部门视图「按成员」着色下，成员 chips 色点与图谱节点颜色对不上。

## 根因与修法

- 序偏移：chips 从调色盘 0 取色 vs adapter 注入序从 1 起 → 错一位
- 单键固化：colorKey 建图时按当时模式写死，切「按成员」不重建图 → reducer
  拿 status 键走 owner 解析落 default → 外来节点全灰
- 修法：双轨键（ownerKey/statusKey 建图双写，reducer 按模式现场选）；
  ownerOrder 单一事实源（GraphPage chips 全序）同时供建图与 chips 色点；
  resolveOwnerColor 导出共用。

## 验证

graph-adapter 5 测（双键共存/注入序/同源解析/兜底/FA2）+ 图谱模块 25 全绿；
浏览器 reducer 插桩实证 s1→owner:0→#7c6bd6（=chips 紫）、l0→owner:1→#c2711d
（=chips 橙）、本人→self→brand。WebGL preserveDrawingBuffer=false 下
getImageData 采样不可靠，行为以 reducer 输出与真机目测为准。
