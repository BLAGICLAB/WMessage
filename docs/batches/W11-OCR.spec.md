# Batch Spec: W11-OCR

## 目的

W11 提交（86f981c）的 OCR 复审修复轮（session `4c5c82ca`，19 条：high 2 / medium 5 / low 12）。
2 high 全修 + 3 medium 顺手修 + 2 WONTFIX（判定见 DEVLOG）。

## 人类可读摘要

- family: workflow-canvas
- 预估 diff: ~7 files / +120/-30 lines
- OCR 计划: 已完成（本批即修复轮）

## 设计

1. **路径闸门进阻塞线程**（2 high 合并，六调用方）：`check_export_path` 的
   canonicalize/symlink_metadata 是同步 fs 调用，原样挂在六个 tauri command 的 async 任务上
   （NFS/USB 卡顿会占死 Tokio worker）——统一挪入 `spawn_blocking_map`（path 克隆进闭包，
   与后续 move 无冲突）。
2. **评审模型下拉 in-flight 守卫**（high）：照同卡 nodeAcceptance 的 busy 模式——连改下拉
   不并发落盘；**弃用乐观更新**，以服务端返回值回填（落盘失败 UI 不展示未持久化值，
   同时修掉"失败回滚由 catch 提示"的失实注释）。
3. **审计哨兵**（medium）：reviewModel 审计值 None → `"-"`（未提交）与 Some("") →
   `"cleared"`（显式清除）可区分。
4. **TOCTOU 留档**（medium）：check_export_path doc 注释显式记录时点检查与实际读写间的
   替换窗口、与 resolve_writable 的口径差异（导出低频+save dialog 路径，风险接受）、
   读侧由 JSON 解析失败兜底。
5. **WONTFIX**：测试临时目录 RAII 清理（断言失败才泄漏，一次性 temp 目录无积累效应）；
   导入路径拆独立闸门放行软链（读侧无写穿透，符号链 .json 罕见，两套闸门的认知成本 >
   收益——留待真实用户诉求）。

## 红线

- 六调用方闸门语义不变（检查链与错误文案不动，只挪线程）
- 下拉落盘以服务端返回值为准；in-flight 期间控件禁用
- 不触碰 evolution 会话的工作区文件（mod.rs/strategy.rs 并行改动，提交精确排除）

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "W11-OCR",
  "family": "workflow-canvas",
  "expected_files": [
    "docs/batches/W11-OCR.spec.md",
    "DEVLOG.md",
    "src-tauri/src/db/tasks.rs",
    "src-tauri/src/db/workflow.rs",
    "src-tauri/src/db/workflow_audit.rs",
    "src-tauri/src/db/workspace.rs",
    "src-tauri/src/db/workflow_settings.rs",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 160,
  "max_lines_removed": 40,
  "findings": [
    {"id": "W11O-1", "file": "src-tauri/src/db/tasks.rs", "line": 1434, "fix": "tasks_export/import 闸门挪 spawn_blocking_map + TOCTOU doc 留档"},
    {"id": "W11O-2", "file": "src-tauri/src/db/workflow.rs", "line": 1074, "fix": "workflow_export/import 闸门挪 spawn_blocking_map"},
    {"id": "W11O-3", "file": "src-tauri/src/db/workspace.rs", "line": 203, "fix": "workspace_export 闸门挪 spawn_blocking_map"},
    {"id": "W11O-4", "file": "src-tauri/src/db/workflow_audit.rs", "line": 242, "fix": "audit_export 闸门挪 spawn_blocking_map"},
    {"id": "W11O-5", "file": "src-tauri/src/db/workflow_settings.rs", "line": 183, "fix": "reviewModel 审计哨兵 None=- / Some(\"\")=cleared"},
    {"id": "W11O-6", "file": "src/components/SettingsPage/SettingsPage.tsx", "line": 3361, "fix": "评审模型下拉 busy 守卫 + 服务端真值回填替代乐观更新"},
    {"id": "W11O-7", "file": "DEVLOG.md", "line": 1, "fix": "reset --hard 事故记录与恢复过程（教训：共享工作区禁用破坏性 git 命令）；mod strategy 声明恢复行最终随 evolution 批次 B-1 提交入库，不在本批 diff"}
  ],
  "assertions_min": {
    "src-tauri/src/db/tasks.rs": 0
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  },
  "stop_conditions": [
    "compile_failure",
    "architecture_blocker",
    "family_heterogeneity",
    "new_high_different_root",
    "gate_fail"
  ]
}
```
