# Batch Spec: U4-POLISH

## 目的

UI 改造战役收官批（依据 `docs/UI-REDESIGN-HANDOFF-2026-09-30.md` U4 定义）：
a11y 基线 + 空态统一 + emoji 图标清尾 + 细节打磨。

1. **a11y 基线（拍板 §2-6）**：
   - **对比度全层级达标**：实测两主题六档文字 token（脚本算 WCAG luminance），
     light t4 4.38/t5 2.53/t6 2.01、dark t5 4.05/t6 2.76 不达 4.5 → 全六档调为
     **双底(bg/surface) ≥4.5:1**（light t4 #525e7a t5 #5d6984 t6 #606c88；
     dark t5 #808a9e t6 #7d879b），层叠单调性保住；底部两档因此贴近——AA 的
     真实代价，token 名保留、视觉档位收窄（main.css 注释说明）。
   - **全局 `:focus-visible`**（brand 2px outline + r-sm 圆角兜底；nm-btn/
     nm-icon-btn/nm-input 的 focus-ring 类选择器优先）；
   - **`prefers-reduced-motion: reduce`** 全局尊重（过渡/动画瞬时化）；
   - 会话下拉 **Esc 关闭**挂触发钮（容器无 tabindex，挂容器键盘不可达——ocr
     复审纠正）。
2. **EmptyState 统一组件**（icon/标题/说明/行动按钮）+ 五处空态接入：
   归档（Archive 图标+归档规则说明）、回收站（Trash2+软删说明）、工作区
   （FolderOpen+**行动按钮**「新建工作区」）、MCP（Plug+**行动按钮**
   「添加服务器」）、技能（FolderInput+引导）。标题文案与既有测试断言一致。
3. **emoji 图标清尾（lucide 收口）**：ChatPanel 家族（Bot/Crosshair/Plus/
   Trash2/Brain/Zap/Square/Copy/FileText/MessagesSquare/Pin）、TodoCard
   （Trash2×3/Undo2×2）、设置面板（ModelRow/SkillsPanel Trash2）、
   EvolutionPanel/DeleteConfirmDialog（Trash2）。**内容标记保留**：📎/🖼️/📁
   （附件/文件夹语义标记，多处测试按文本断言）与 TodoCard 的 📌锚/🎯 等
   功能性 emoji（有专项语义，U4 范围外）。
   `isImagePath`+IMAGE_EXTS 自 UserBubbleContent 归位 `format.ts`（纯路径
   谓词与 basename 同属，ocr U3b 建议落地）。
4. 测试断言随图标同步（语义不变）：ChatPanel.test（🤖 默认会话→文本、
   ⚡/💬/🧠/＋ 去 emoji、模型 chip 断言改 toHaveTextContent 防同名多重）、
   TodoCard.test（↩ 恢复→恢复）、WorkspacePage.test（空态正则）、
   DeleteConfirmDialog.test（标题文本）。

## 红线核对

- 主题机制/双窗口/src-tauri/看板拖拽零改动；nm-* 类名零改动。
- 全部测试 **350 绿**（45 文件，+2 EmptyState；全量连跑两次稳定，ChatPanel
  会话过滤用例存在已知全量时序敏感、复跑即过）；oxlint 0 warn；knip 0；tsc 过。
- **test-all 全量（含 Rust + 审计）exit 0（110s）**。
- 流式性能不回退：perf 数据 U3b 35.9×~85.5× → 本批 30.1×~90.5× 波动带内。

## 验收记录（视觉终审）

夹具重建（stub invoke + 真实 App 挂载，含带子任务/截止的看板数据），
web-gui-tester 矩阵截图（`gui-test-screenshots/u4_*.png`，不入库）：
深色看板（全功能任务卡+导航）、深色归档（归档卡）、深色回收站（**EmptyState
实拍**：软底圆图标+标题+说明居中落位）、浅色看板（对比度提亮后 t4-t6 文字
明显更清晰）。深色聊天/浅色设置等页面 U1~U3b 各批已留档；主题机制本批
零改动。

## ocr review 复审处置记录

10 条（0H/4M/6L，含重复去重后 4 独立项）：

- **medium 4 修**：Undo2 `mr-1 inline` 间距一致性反哺 Trash2（并去掉 inline
  改 flex 容器语义）；全局 :focus-visible 注释校准（focus-ring 系统词）+
  补 r-sm 圆角；Esc 关闭从下拉容器（无 tabindex 键盘不可达）迁到触发按钮
  （含冒泡，鼠标开菜单也能 Esc）；（第 4 条与首条同点位去重）。
- **low 6**：记 6（EmptyState icon aria-hidden 已带、断言同步说明、
  border-radius 组合顺序等微项）。

## spec 起草后自查三条

1. expected_files = staged 全集 21 项（含本 spec）。
2. 预算：M 文件 numstat 合计 +212/-107；新文件 EmptyState.tsx 41 行 +
   EmptyState.test.tsx 36 行 + 本 spec。
3. findings 留 1 条：ChatPanel 会话过滤用例全量跑时序敏感（既有注释在案，
   隔离跑与复跑均绿），随 U4 后续或独立小批治理。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U4-POLISH",
  "family": "ui-redesign",
  "expected_files": [
    "docs/batches/U4-POLISH.spec.md",
    "src/components/ArchivePage.tsx",
    "src/components/ChatPanel.test.tsx",
    "src/components/ChatPanel/InputArea.tsx",
    "src/components/ChatPanel/MessageList.tsx",
    "src/components/ChatPanel/SessionList.tsx",
    "src/components/ChatPanel/UserBubbleContent.tsx",
    "src/components/EmptyState.test.tsx",
    "src/components/EmptyState.tsx",
    "src/components/EvolutionPanel/DeleteConfirmDialog.test.tsx",
    "src/components/EvolutionPanel/DeleteConfirmDialog.tsx",
    "src/components/EvolutionPanel/EvolutionPanel.tsx",
    "src/components/SettingsPage/McpPanel.tsx",
    "src/components/SettingsPage/ModelRow.tsx",
    "src/components/SettingsPage/SkillsPanel.tsx",
    "src/components/TodoCard.test.tsx",
    "src/components/TodoCard/TodoCard.tsx",
    "src/components/TrashPage.tsx",
    "src/components/WorkspacePage.test.tsx",
    "src/components/WorkspacePage.tsx",
    "src/format.ts",
    "src/ui/main.css"
  ],
  "max_lines_added": 380,
  "max_lines_removed": 160,
  "max_new_files_lines": 240,
  "findings": [
    { "file": "src/components/ChatPanel.test.tsx", "note": "会话过滤用例全量跑时序敏感（隔离跑/复跑均绿，既有注释在案），登记后续治理" }
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
