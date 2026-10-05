# Batch Spec: G4-CLUSTER / G5-DEPEDIT / G6-SYNONYM 合并批

```json
{
  "batch_id": "G4-G6-AFFINITY",
  "family": "task-graph",
  "expected_files": [
    "docs/batches/G4-G6-AFFINITY.spec.md",
    "docs/TASK-GRAPH-AFFINITY-DEPS-2026-10-05.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/tag_similar.rs",
    "src-tauri/src/lib.rs",
    "src/components/GraphPage/GraphCanvas.tsx",
    "src/components/GraphPage/GraphPage.tsx",
    "src/components/GraphPage/GraphPage.test.tsx",
    "src/components/GraphPage/graph-adapter.ts",
    "src/components/GraphPage/graph-adapter.test.ts",
    "src/components/GraphPage/graph-build.ts",
    "src/components/GraphPage/graph-build.test.ts",
    "src/App.tsx",
    "DEVLOG.md"
  ],
  "max_lines_added": 900,
  "max_lines_removed": 200,
  "findings": [
    {"id": "G4-1", "file": "src/components/GraphPage/graph-adapter.ts", "line": 1, "fix": "确定性分扇区初值（同标签/同义组专属扇区）+ 锚点扇区中心质点 + TAG_EDGE_WEIGHT/组数下限常量"},
    {"id": "G4-2", "file": "src/components/GraphPage/GraphCanvas.tsx", "line": 1, "fix": "锚点隔离（hidden/事件/边）+ fitToContent 视野自适应（重建粗适配 + 收敛终态瞬时就位）"},
    {"id": "G5-1", "file": "src/components/GraphPage/graph-build.ts", "line": 1, "fix": "wouldCreateDepCycle 纯函数（DFS 可达即环）"},
    {"id": "G5-2", "file": "src/components/GraphPage/GraphPage.tsx", "line": 1, "fix": "详情面板依赖编辑（仅本人卡/候选过滤/环提示/×移除）+ onPatchTask 通道 + tagGroups 近义接线"},
    {"id": "G6-1", "file": "src-tauri/src/tag_similar.rs", "line": 1, "fix": "tag_similar_pairs（embed_text + 向量缓存 + 余弦阈值 0.78 + 上限 200 + 审计）"}
  ],
  "assertions_min": {
    "src/components/GraphPage/graph-adapter.test.ts": 0,
    "src/components/GraphPage/graph-build.test.ts": 0
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 5
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

老板验收反馈三连（设计 docs/TASK-GRAPH-AFFINITY-DEPS-2026-10-05.md）：
① 同标签节点聚拢 + 点标签过滤后散开铺满；② 普通任务卡支持依赖连线（编辑入口）；
③ 标签近义合并聚簇。拖线建边不做。

## 实现要点

- **G4**：力学聚簇被压测证伪（锚点星型边 weight 0.35/3、linLogMode 全试过，
  平衡态 inAvg/outAvg ≈ 0.96）→ 改**确定性分扇区初值**（同组节点建图时铺在
  专属扇区小螺旋，无组节点全局螺旋环绕；锚点=组中心隐藏大 size 质点维持凝聚）。
  视野自适应：重建后 400ms 粗适配 + FA2 收敛停机时瞬时就位（duration 0，规避
  rAF 节流冻结）。锚点隔离四路径：hidden 渲染/边隐藏/事件忽略/度数用建图值。
- **G5**：`wouldCreateDepCycle`（depId 正向可达 selfId 即环）+ 详情面板依赖区
  （仅本人卡、候选五重过滤、行内环提示）→ onPatchTask（App updateTask →
  task_patch）。零新后端。
- **G6**：`tag_similar_pairs(tags)`——spawn_blocking + embed_text（512 维 L2）
  + 进程级向量缓存（512 上限清空）+ 点积 ≥0.78（上限 200 对）+ 审计。
  前端并查集并组 → tagGroups → 锚点按组建。

## 验证

图谱模块 31 测全绿（锚点/权重/环检测/双键）；Rust tag_similar 3 测；vitest
442 + test-fast + build 全绿。压测 1227：聚簇度 0.46（同组距离近 54%）、
~25s 收敛自动停机、视野自适应铺满（graph-cluster-1227.png）。
