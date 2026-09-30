# Batch Spec: U1-TOKEN

## 目的

UI 改造战役第一批（依据 `docs/UI-REDESIGN-HANDOFF-2026-09-30.md` U1 定义）：
把「新拟态凸起卡片」换成「Linear 式扁平分层」设计语言的**换肤地基**。

1. `src/ui/main.css` token 重写：三层表面 `--bg/--surface/--surface-raised`（亮度分档）、
   1px 边框语义（`--edge`/`--edge-strong`）、圆角梯度 `--r-sm/md/lg`（8/12/16）、
   字号阶梯 `--fs-xs..lg`（13/14/15/16，供新组件取 var；存量 Tailwind 类与四档字号
   拍板机制不动）、单层柔和投影 `--shadow-sm`、键盘焦点环 `--focus-ring`、强调色收敛
   （success 绿 + 低饱和蓝紫灰品牌色保留）。
2. `.nm-card/.nm-card-hover/.nm-inset/.nm-outset/.nm-btn/.nm-sidebar-panel(-top)/
   .nm-task-title` **同名重实现**为扁平分层材质（类名零改动，上层组件零改动换肤）。
   悬停语义换为「背景抬升 + 边框提亮」，移除 neumorphic 双阴影与 translateY 上浮。
3. 悬空类名补实现：`.nm-input`（McpPanel 输入已在用但此前无定义）、`.nm-tag`
   （EvolutionPanel 徽章同前）；新增 `.nm-icon-btn`（IconButton 基材）。
4. 引入 `lucide-react`（唯一新依赖，MIT，tree-shake）；`IconButton` 基础件
   （type=button 默认、nm-icon-btn 材质、className 透传）；顺手替换 emoji 图标：
   WorkspacePage 删除/编辑 ×3、McpPanel 编辑/删除 ×2（大范围替换留 U4）。
5. `main.css.test.ts` 过渡对称断言迁移到新材质语义（基类 transition 声明 hover
   变化的属性、hover 块无 transform/box-shadow 跳变）——查样式的既有断言，语义
   「hover 过渡对称」不变，正则随新 transition 声明更新。

## 红线核对

- 主题机制 `src/theme.ts` 零改动（双窗口 storage 同步原样）；`src-tauri/` 零触碰。
- 看板拖拽、挂件交互零改动（本批仅 css 材质 + IconButton 替换，不动任何交互逻辑）。
- oxlint 32 warn 存量持平、0 error；knip exit 0；tsc 通过；vitest 341 全绿。
- 四档字号机制（`[data-font-size]`）、滚动条双机制（main-css.test.ts 锁死）、
  md-body 排版规则（老板拍板段落间距）逐字保留。

## 测试与验收

- 定向：vitest 全量 341 绿（+2 IconButton 新测试）/ tsc / oxlint / knip。
- 视觉：`npm run dev` + web-gui-tester 黑盒截图，浅/深/跟随系统三主题 ×
  看板/归档/工作区/回收站/设置 五页 + nm-input 焦点环 + 工作区卡（nm-card +
  lucide IconButton 实渲染）。纯浏览器模式下挂件（聊天）窗口因**存量** Tauri
  耦合无法启动（WidgetApp 无 ErrorBoundary + `getCurrentWindow()` 直调，基线
  commit 同样复现，非本批回归），聊天页换肤由 nm-* 同名重实现覆盖、随 U3 批在
  Tauri 环境补验；截图存 `gui-test-screenshots/`（本地验收材料，不入库）。
- 主题切换走设置页真实芯片（浅→深→跟随系统），持久化与解析逐项核对。

## ocr review 复审处置记录

15 条意见（1 HIGH / 8 MED / 6 LOW）：

- **HIGH** `.nm-btn` 缺 `:focus-visible`——当场修（--focus-ring 统一配方，同步补
  `.nm-icon-btn`/`.nm-input` 焦点态）。
- MED 已修：`--inset-bg` 浅色 0.045→0.075（选中/凹陷可读性）、浅色 `--t6`
  #bcc5d6→#9fadc4（1.3:1→~2.3:1，**部分处理**，全量 AA 对比度复核按拍板留 U4
  a11y 批）、`@apply rounded-*` 接线到 `--r-*` token、disabled 态 `cursor:
  not-allowed` + hover 收窄 `:not(:disabled)`、body 主题切换过渡 0.28s→0.15s
  与组件层同步、`--shadow-lg` 死 token 删除（U2 ⌘K 浮层落地时随浮层组件引回）。
- LOW 已修：IconButton className 尾空格、McpPanel 图标按钮补 `aria-label`、
  hover 块冗余 transition/box-shadow 槽删除、侧栏面板过渡 0.28s→0.15s、
  `.nm-tag` 注释标明静态件、`--r-*` 接线（同 MED 去重）。
- 驳回 1 条（LOW）：建议 `.nm-card-hover:hover` 加回 transform 位移/阴影抬升——
  与 Linear 扁平悬停纪律及「无弹跳」红线冲突，悬停信号改为背景+边框已足够。

## spec 起草后自查三条

1. expected_files = staged 全集 9 项（**含 spec 文件本身**，B6 教训）。
2. 预算：M 文件 numstat 合计 +212/-114（main.css +150/-84、package-lock +16/-0、
   WorkspacePage +14/-12、McpPanel +12/-8、main.css.test.ts +16/-9、package.json
   +4/-1）；新文件 IconButton.tsx 25 行 + IconButton.test.tsx 36 行 + spec 本文件。
3. findings 留 1 条：浅色 `--t6` 对比度为部分处理（1.3:1→~2.3:1），全量 WCAG AA
   复核在 U4 a11y 批收口（拍板 §2-6）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U1-TOKEN",
  "family": "ui-redesign",
  "expected_files": [
    "package.json",
    "package-lock.json",
    "src/components/SettingsPage/McpPanel.tsx",
    "src/components/WorkspacePage.tsx",
    "src/ui/IconButton.test.tsx",
    "src/ui/IconButton.tsx",
    "src/ui/main.css",
    "src/ui/main.css.test.ts",
    "docs/batches/U1-TOKEN.spec.md"
  ],
  "max_lines_added": 260,
  "max_lines_removed": 150,
  "max_new_files_lines": 260,
  "findings": [
    { "file": "src/ui/main.css", "note": "浅色 --t6 对比度部分处理（1.3:1→2.3:1），全量 AA 复核留 U4 a11y 批" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx --no-install vitest --run          # 341 绿
npx --no-install tsc --noEmit -p tsconfig.json
npx --no-install oxlint src            # 32 warn（存量持平）/ 0 error
npx --no-install knip --no-progress    # exit 0
```
