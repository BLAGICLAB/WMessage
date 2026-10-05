# Batch Spec: G3-SIGMA 图谱渲染层迁移（Sigma.js + FA2 worker）

```json
{
  "batch_id": "G3-SIGMA",
  "family": "task-graph",
  "expected_files": [
    "docs/batches/G3-SIGMA.spec.md",
    "src-tauri/tauri.conf.json",
    "src/components/GraphPage/GraphCanvas.tsx",
    "src/components/GraphPage/graph-adapter.ts",
    "src/components/GraphPage/graph-adapter.test.ts",
    "src/components/GraphPage/GraphPage.tsx",
    "src/components/GraphPage/GraphPage.test.tsx",
    "src/components/GraphPage/physics.ts",
    "src/components/GraphPage/physics.test.ts",
    "src/test/setup.ts",
    "package.json",
    "package-lock.json",
    "DEVLOG.md"
  ],
  "max_lines_added": 800,
  "max_lines_removed": 700,
  "findings": [
    {"id": "G3-S1", "file": "src/components/GraphPage/GraphCanvas.tsx", "line": 1, "fix": "Sigma v3 WebGL 渲染 + FA2 worker + reducer 交互全保 + 收敛自动停机"},
    {"id": "G3-S2", "file": "src/components/GraphPage/graph-adapter.ts", "line": 1, "fix": "graphology 适配层（colorKey 语义着色/size 度数半径/FA2 设置按规模推导）"},
    {"id": "G3-S3", "file": "src-tauri/tauri.conf.json", "line": 1, "fix": "CSP 补 worker-src/script-src blob:（FA2 supervisor Blob-URL worker）"},
    {"id": "G3-S4", "file": "src/test/setup.ts", "line": 1, "fix": "jsdom 补 WebGL2/WebGL 构造存根（sigma 顶层特性探测）"}
  ],
  "assertions_min": {
    "src/components/GraphPage/graph-adapter.test.ts": 0
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  },
  "stop_conditions": [
    "compile_failure",
    "gate_fail"
  ]
}
```

## 目的

一年近万条任务的产品规模下，把图谱渲染层从自研 Canvas2D（舒适区 3~5k 节点）
迁移到 Sigma.js v3 WebGL + ForceAtlas2 worker。老板拍板：不保留旧渲染器、
不做 worker 降级。数据模型/graph-build/过滤器/详情面板零改动（迁移只发生在
渲染层，graph-build.ts 的渲染无关设计兑现）。

## 删除清单

- `physics.ts`（自研力导向物理，~300 行）与其测试——FA2 替代
- 旧 GraphCanvas 的 Canvas2D 绘制层（脏检查/rAF 循环/批量分桶）
- `@dagrejs/dagre` **保留**（工作流编辑器仍用）

## 已知验收口径

- FPS 数值无法在 IAB harness 采样（面板非前台 rAF 节流），帧流畅度由老板真机验收
- 万节点压测数据为恶意密集（1/3 done + 每 hub 200 成员），真实数据密度低得多
- 主线程过滤点击：1227 档 0~1.6ms、10027 档 0.5ms（验收标准 <5ms 达标）
