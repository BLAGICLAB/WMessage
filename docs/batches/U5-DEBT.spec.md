# Batch Spec: U5-DEBT

## 目的

UI 改造战役挂账治理批（U1–U4 登记项收口，依据 DEVLOG 各批登记）：

1. **会话相对时间**（U3b 登记项，判断修正）：侦查发现 Rust `BotSession`
   **本就序列化 createdAt/updatedAt**（`#[serde(rename_all = "camelCase")]`，
   bot_sessions_load 返回即带）——U3b findings「需 src-tauri 扩展」判断有误，
   **src-tauri 零改动**。前端：Session 类型 +可选时间戳（chat-open-session 前端
   补行的会话不带，按无时间渲染）；`format.ts` +`relativeTime`（刚刚/N 分钟前/
   N 小时前/N 天前/超 7 天落日期，now 可注入，非法/未来回「刚刚」）；SessionList
   下拉行 ml-auto 弱色 10px 相对时间。
2. **挂件 ErrorBoundary**（U1 登记项）：App.tsx 的 ErrorBoundary 类提取到
   `src/ui/ErrorBoundary.tsx` 共享（文案「主窗口」→「窗口」），WidgetApp 根
   包裹——挂件异常不再白屏。ocr medium 修：非 Error throw 运行时规整
   （message/stack instanceof 守卫）。
3. **纯浏览器 alert 洪水**（U1 登记项）：errorHandler 加 `isTauriHost()`
   检测（`__TAURI_INTERNALS__` 探测）——非 Tauri 宿主一律只 console（弹窗/
   confirm 降级），Tauri 宿主行为不变。全局 `setup.ts` 统一模拟 Tauri 宿主
   （=产品真实运行环境），errorHandler.test 显式 delete 后单测降级分支
   （+2 测试）。
4. **ChatPanel 会话过滤用例时序敏感**（U4 登记项）：根因=sessionIdRef 镜像在
   passive effect，findByText 走 MutationObserver 可在 effects 前解析（慢机
   窗口大）——fire 前 `await act(async () => {})` 显式 flush，注释更新。

## 红线核对

- **src-tauri 零改动**（相对时间不需要后端——修正 U3b findings 的判断）；
  主题机制/双窗口/看板拖拽零改动。
- 全部测试 **354 绿**（45 文件，+4：relativeTime×2 + 宿主降级×2）；
  oxlint 0 warn；knip 0；tsc 过；**test-all 全量含 Rust+审计 exit 0（70s）**；
  perf 89.3×（波动带上沿）。

## ocr review 复审处置记录

2 条（0H/1M/1L）：medium 修（ErrorBoundary 非 Error throw 规整——instanceof
守卫 message/stack）；low 记 1（SessionList 相对时间测试未单列——SessionList
无独立测试文件，行为由 relativeTime 单测 + ChatPanel 集成覆盖）。

## spec 起草后自查三条

1. expected_files = staged 全集 12 项（含本 spec）。
2. 预算：M 文件 numstat 合计约 +120/-50；新文件 ErrorBoundary.tsx（约 60 行）+
   本 spec。
3. findings 留 1 条：U3b spec 中「会话相对时间需 src-tauri 扩展」判断有误，
   本批修正（BotSession 本就带时间戳）；+/- diff 统计仍需后端事件面扩展，
   维持登记。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U5-DEBT",
  "family": "ui-redesign",
  "expected_files": [
    "docs/batches/U5-DEBT.spec.md",
    "src/App.tsx",
    "src/components/ChatPanel.test.tsx",
    "src/components/ChatPanel/SessionList.tsx",
    "src/components/ChatPanel/types.ts",
    "src/components/WidgetApp/WidgetApp.tsx",
    "src/format.test.ts",
    "src/format.ts",
    "src/lib/errorHandler.test.ts",
    "src/lib/errorHandler.ts",
    "src/test/setup.ts",
    "src/ui/ErrorBoundary.tsx"
  ],
  "max_lines_added": 300,
  "max_lines_removed": 140,
  "max_new_files_lines": 150,
  "findings": [
    { "file": "src/components/ChatPanel/SessionList.tsx", "note": "U3b findings 修正：会话相对时间不需要 src-tauri 扩展（BotSession 本就序列化 createdAt/updatedAt）；+/- diff 统计仍待后端事件面扩展" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
bash scripts/test-all.sh               # exit 0（含 Rust + 审计）
npx --no-install tsc --noEmit -p tsconfig.json
npx --no-install oxlint src            # 0 warn / 0 error
npx --no-install knip --no-progress    # exit 0
```
