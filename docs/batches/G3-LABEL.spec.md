# Batch Spec: G3-LABEL 视觉修订——标签配色/光晕/主题化 hover

```json
{
  "batch_id": "G3-LABEL",
  "family": "task-graph",
  "expected_files": [
    "docs/batches/G3-LABEL.spec.md",
    "src/components/GraphPage/GraphCanvas.tsx",
    "gui-test-screenshots/graph-sigma-labels-light.png",
    "gui-test-screenshots/graph-sigma-labels-dark.png",
    "gui-test-screenshots/graph-sigma-labels-hover.png"
  ],
  "max_lines_added": 260,
  "max_lines_removed": 80,
  "findings": [
    {"id": "G3-L1", "file": "src/components/GraphPage/GraphCanvas.tsx", "line": 1, "fix": "LABEL_COLORS 双主题色阶 + 自定义 label/hover 绘制（光晕衬底/主题胶囊底板/中文字体栈/labelColor 属性通道）"}
  ],
  "assertions_min": {
    "src/components/GraphPage/GraphCanvas.tsx": 0
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

老板验收反馈：中文标签颜色太黑不美观（亮色黑字压彩点、暗色下 sigma 默认
#000 直接不可读、hover 底板写死白底突兀）。重设计标签视觉，不改任何行为。

## 设计要点

- 画布标签与 UI 文本 token 解耦：专用 LABEL_COLORS（亮/暗），常态 = 雾蓝灰
  "路牌"层次、hub = 品牌色、焦点 = 提亮一档
- 自定义 defaultDrawNodeLabel：底色同色半透明光晕衬底（抹掉底层图形而非盖住）
- 自定义 defaultDrawNodeHover：胶囊几何照抄官方，底色/阴影/文字按主题
- 字体栈补 PingFang SC / Microsoft YaHei；labelColor 走 { attribute } 通道，
  主题切换即时生效
