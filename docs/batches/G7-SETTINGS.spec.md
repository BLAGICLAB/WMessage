# Batch Spec: G7-SETTINGS 设置页「任务图谱」模块（七项图谱偏好）

```json
{
  "batch_id": "G7-SETTINGS",
  "family": "task-graph",
  "expected_files": [
    "docs/batches/G7-SETTINGS.spec.md",
    "DEVLOG.md",
    "src/lib/graphPrefs.ts",
    "src/lib/graphPrefs.test.ts",
    "src/components/GraphPage/GraphCanvas.tsx",
    "src/components/GraphPage/GraphPage.tsx",
    "src/components/GraphPage/graph-adapter.ts",
    "src/components/GraphPage/graph-adapter.test.ts",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/SettingsPage/GraphSettingsPanel.tsx",
    "src/components/SettingsPage/GraphSettingsPanel.test.tsx"
  ],
  "max_lines_added": 200,
  "max_lines_removed": 50,
  "findings": [
    {"id": "G7S-1", "file": "src/lib/graphPrefs.ts", "line": 1, "fix": "wm.graph.* 七项偏好单一事实源（localStorage；非法值/损坏 JSON 回退默认；记住过滤器关掉即清除存档）；图谱页仅主窗口存在，无跨 webview 同步需求"},
    {"id": "G7S-2", "file": "src/components/GraphPage/GraphPage.tsx", "line": 51, "fix": "过滤器初始化优先级：记住上次 > 只看我的 > 默认；开启记住时变更即持久化"},
    {"id": "G7S-3", "file": "src/components/GraphPage/GraphCanvas.tsx", "line": 110, "fix": "labelDensity/edgeWidth 走 reducer（propsRef 现读 + refresh 不重建）；autoLayout 门控 FA2 初启与拖拽续跑；looseness 建图输入切换即重建"},
    {"id": "G7S-4", "file": "src/components/GraphPage/graph-adapter.ts", "line": 70, "fix": "GraphLooseness + LOOSENESS_R_MAX 系数 20/30/45（内部常量，knip 抓掉多余 export）"},
    {"id": "G7S-5", "file": "src/components/SettingsPage/GraphSettingsPanel.tsx", "line": 1, "fix": "设置页第十一分类「任务图谱」面板：SwitchRow/SegmentedRow，即时生效无保存按钮"}
  ],
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

设置页新增「任务图谱」分类，七项偏好（老板拍板清单）：只看我的任务 /
节点大小（连接度·耗时）/ 标签密度（少·标准·多）/ 连线粗细（细·标准·粗）/
自动播放布局动画 / 布局松散度（紧凑·标准·松散）/ 记住上次的过滤器。
全部纯前端偏好（localStorage `wm.graph.*`），零 Rust 改动。
