# Batch Spec: G3-PERF 图谱大图性能（修卡死/崩溃）

```json
{
  "batch_id": "G3-PERF",
  "family": "task-graph",
  "expected_files": [
    "docs/batches/G3-PERF.spec.md",
    "src/components/GraphPage/physics.ts",
    "src/components/GraphPage/GraphCanvas.tsx",
    "src/components/GraphPage/graph-build.ts",
    "src/components/GraphPage/physics.test.ts",
    "DEVLOG.md"
  ],
  "max_lines_added": 450,
  "max_lines_removed": 220,
  "findings": [
    {"id": "G3-P1", "file": "src/components/GraphPage/GraphCanvas.tsx", "line": 256, "fix": "脏检查跳过分支误留 rAF 自调度 + draw 外层各排一份 → 每帧回调数翻倍指数爆炸，冷却后成千排队回调各做全量重绘 = 大图卡死/崩溃根因；paintFrame 永不自调度"},
    {"id": "G3-P2", "file": "src/components/GraphPage/physics.ts", "line": 1, "fix": "空间网格模块级复用（每 tick 建 1 次共用斥力+碰撞，原 3 次）+ preSettle 毫秒预算（固定 150 tick → 24ms 预算自动砍 tick）"},
    {"id": "G3-P3", "file": "src/components/GraphPage/GraphCanvas.tsx", "line": 1, "fix": "批量绘制：边 4 桶 + 箭头子路径单次 fill + 节点颜色×透明层分桶；大图标签上限（>400 节点仅高连接度/焦点/邻域）"},
    {"id": "G3-P4", "file": "src/components/GraphPage/graph-build.ts", "line": 1, "fix": "workflows 线性查找 → Map O(1)"}
  ],
  "assertions_min": {
    "src/components/GraphPage/physics.test.ts": 0
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 3
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

修真实数据量下图谱「好卡、点筛选崩溃」：多人汇总 + 工作流后节点数从演示级 27 涨到
千级，暴露四个性能缺陷（G3-P1 为崩溃根因）。1227 节点 / 804 依赖压测通过：
过滤器点击同步耗时 0~0.8ms（修前无限挂死），布局 6s 内收敛，批量绘制 + 标签上限
后成图可读（gui-test-screenshots/graph-07-stress-1200.png）。

## 红线

- 物理行为不变（斥力/弹簧/向心/碰撞常数不动；碰撞从每 tick 2 次迭代降为 1 次——
  tick 内位移 ≪ cutoff，防重叠效果等价）
- 视觉输出不变（分桶绘制聚合同样式图元，透明层/颜色逐点语义保留）
- v1 交互全保留（hover/选中/拖拽固定/指针锚缩放/双击 hub）
