# Batch Spec: U20C-ICON2

## 目的

图标迁移批 4（方案 `docs/UI-ICON-PLAN-2026-10-04.md`）：**错误弹窗组件化**——
errorHandler 的 ❌💡🔁 依赖原生 alert/confirm，原生弹窗无富文本能力。本批把
错误提示迁到应用内 nm 卡片弹窗（ErrorDialogHost），❌→CircleX、💡→Lightbulb、
🔁→RotateCcw 由 lucide 承载。

## 设计（调用面零改动）

- `handleCommandError` 签名不变（30+ 调用点不动）；内部改为发
  `ERROR_DIALOG_EVENT`（CustomEvent，cancelable），`ErrorDialogHost` 监听并
  **preventDefault 声明接管**，渲染图标化卡片；`resolve(retry)` 回调驱动重试。
- **双窗口各挂一个 Host**：App.tsx（主窗）+ WidgetApp.tsx（挂件；挂在根节点
  常驻——折叠/展开两态都在，首版曾误放 expanded 分支被测试抓出）。
- **原生兜底保留**：未挂 Host 的窗口（理论不可达 + 非常规宿主）走原生
  alert/confirm 旧 emoji 格式——原生弹窗无富文本能力，兜底路径的视觉语言
  保留；存量 errorHandler.test.ts（全部走兜底路径）**零改动通过**。
- onRetry 同步抛错回收语义不变（silent 回收入同一入口）。

## 测试

- 新增 `src/ui/ErrorDialogHost.test.tsx` 5 用例：不可恢复卡片形态（message+hint
  行+确定、无 emoji 文案）、重试路径（onRetry 恰好一次+弹窗关）、取消路径、
  onRetry 抛错回收、无 Host 原生兜底旧格式。
- 存量适配 2 文件：App.test（db_load 读失败/mutate 落盘失败 → 断言
  alertdialog 内容）；WidgetApp.test（轮询报错 2 用例 → dialog 断言 +
  ERROR_DIALOG_EVENT 请求数计「每周期都上报」；fake timers 下 act 包裹推进、
  同步 getByRole——findByRole 的 waitFor 真实计时器会死锁）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U20C-ICON2",
  "family": "ui-icon-migration",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U20C-ICON2.spec.md",
    "src/App.test.tsx",
    "src/App.tsx",
    "src/components/WidgetApp.test.tsx",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/lib/errorHandler.ts",
    "src/ui/ErrorDialogHost.test.tsx",
    "src/ui/ErrorDialogHost.tsx"
  ],
  "max_lines_added": 400,
  "max_lines_removed": 80,
  "max_new_files_lines": 280,
  "findings": [
    { "file": "src/lib/errorHandler.ts", "note": "事件桥设计：CustomEvent cancelable + preventDefault 接管协议；签名不变调用面零改动；原生兜底旧格式保留（errorHandler.test 零改动通过）" },
    { "file": "src/ui/ErrorDialogHost.tsx", "note": "nm 卡片 alertdialog（CircleX/Lightbulb/RotateCcw）；resolve 恰好一次；overlay 点击=取消" },
    { "file": "src/components/WidgetApp/WidgetApp.tsx", "note": "Host 挂根节点常驻（折叠/展开两态都在；曾误放 expanded 分支被测试抓出修正）" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx vitest --run                                  # 406/406
bash scripts/test-fast.sh                         # exit 0
python3 scripts/batch-verify.py docs/batches/U20C-ICON2.spec.md
```
