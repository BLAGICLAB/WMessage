# Batch Spec: U20C-ICON1B

## 目的

U20C-ICON1 终扫补漏（老板要求复查残余 emoji）：挂件任务卡编排区的**预算徽标
⏱**（maxTurns 轮数 / maxWallSeconds 墙钟）是批 1 漏掉的一处 DOM 符号——补
lucide `Timer size=10` + `inline-flex items-center gap-1 whitespace-nowrap`
（对齐批 1 规范）；测试断言适配（图标 aria-hidden 后文本节点为纯
「30轮·600s」）。

## 终扫结论（本批同时交付的全量盘点）

前端 DOM/字符串层余 38 行、后端非注释余 36 行，全部为方案保留项：
- 前端：聊天消息正文 4（内容纪律）、addHint 字符串 6（string API 随未来
  hint 组件化再议）、🧩 判定兜底 1（批 5 兼容回退，必须保留）、errorHandler
  原生兜底 4（批 4 口径）、ConfirmMap/yolo 确认文案 3（string prop）、句内
  单色 ✓ 5 与 ← 返回 1（文本符号）、⌘K/⌘N 键盘符号 2（平台正确语义）、
  块注释续行 11；
- 后端 36：聊天消息话术（批量汇总/失败提示/子任务推进/定时完成）、LLM
  提示词来源标记（prompts/execute.rs）、进聊天流的闸门错误文案、审计分类
  前缀判定（audit.rs `starts_with("⚠️")`——功能性契约同 🧩）、CLI 终端
  ✓/✗（终端单色渲染）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U20C-ICON1B",
  "family": "ui-icon-migration",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U20C-ICON1B.spec.md",
    "src/components/TaskCardContent.test.tsx",
    "src/components/TaskCardContent.tsx"
  ],
  "max_lines_added": 40,
  "max_lines_removed": 20,
  "max_new_files_lines": 80,
  "findings": [
    { "file": "src/components/TaskCardContent.tsx", "note": "预算徽标 ⏱ → Timer size=10 + inline-flex nowrap（批 1 规范对齐）；title 悬停完整预算不变" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx vitest --run                # 406/406
bash scripts/test-fast.sh       # exit 0
```
