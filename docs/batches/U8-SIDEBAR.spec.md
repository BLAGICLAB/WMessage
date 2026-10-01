# Batch Spec: U8-SIDEBAR

## 目的

老板需求（U7 设置壳的分组细化）：侧栏改为十项分类，原「机器人」大卡拆分——
授权模式留在机器人、两个搜索引擎（Tavily/Brave）做成 MCP 服务并让客户可选
是否开启。

1. **十项侧栏**（SECTIONS 重排）：通用设置 / 数据管理 / 机器人 / 模型设置 /
   记忆 / 技能 / MCP 服务 / 自进化 / 桌面整理 / 词元统计（各配 lucide 图标）。
   内容映射（源码序 = 侧栏序，零块搬移）：通用=个人资料+通用设置；数据管理=
   任务数据+工作区管理；机器人=三卡（开关/Python 三件/审计日志 ‖ 授权模式+
   文件白名单 ‖ 智能技能路由+外部 API+token）；模型设置=大模型 API 配置
   （botEnabled 门保留）+保存钮+推理强度；记忆=记忆整理；技能=SkillsPanel；
   MCP 服务=Tavily/Brave 搜索引擎卡（开关可选开启，互斥提示保留）+保存钮+
   McpPanel；自进化=EvolutionPanel；桌面整理=MigrationPanel（其标题本即
   「桌面清理」）；词元统计=EmptyState 诚实占位（全仓无 token 用量数据源，
   见 findings）。
2. **同名 section 多次出现**：「机器人」section 三段、「MCP 服务」两段——
   同 key 同亮，零 JSX 块搬移（原 botEnabled 条件块在 Key 段后闭合，授权/
   推理/白名单/Tavily/Brave/保存钮切出为恒显或独立门控）。
3. **持久化回归修复**（ocr HIGH×3 采纳）：拆卡后 Python 超时/白名单 textarea/
   技能路由等 setConfig 字段失去保存路径（原靠大卡单一保存钮，model 卡按钮
   又被 botEnabled 门挡住）——bot 三卡各补「保存配置」钮（renderSaveButton
   局部函数消 5 处复制粘贴）；技能路由按钮补 configBusy 守卫（ocr medium）。
4. 测试同步：初始渲染用例改十分类逐项断言；模型/Tavily/推理/导出/导入用例
   导航到新分类名；两处同名文本改 getAllByText。

## 红线核对

- 各功能块内部逻辑/state/invoke 面零改动（搬运仅为 JSX 包裹层）；双窗口/
  主题机制/看板拖拽零改动；src-tauri 零改动。
- 全部测试 **354 绿**；oxlint 0 warn；knip 0；tsc 过；perf 波动带内。

## 验收记录（视觉）

dev shim 实拍（`gui-test-screenshots/u8_*.png`）：深色十项侧栏全清单（图标+
选中态）、「MCP 服务」页（Tavily 已开启/Brave 已关闭两张搜索引擎卡+互斥提示+
保存配置+McpPanel 空态 EmptyState）、「自进化」分类切换实测。

## ocr review 复审处置记录

9 条（5H/3M/1L）：**HIGH×3 采纳**——拆卡持久化回归（Python 超时/白名单/
技能路由无保存路径），bot 三卡各补保存钮+renderSaveButton 去重；**HIGH×2
驳回/记录**——「bot 三段各自 nm-card」为拆卡设计意图；「授权/白名单原在
botEnabled 门内现恒显」为有意行为（设置页可预配置，机器人关闭时设置仍可见，
各卡自有保存钮不再依赖门）；**medium 2**：技能路由补 configBusy（修）、
保存按钮复制粘贴→renderSaveButton（修）；low 1 记。

## spec 起草后自查三条

1. expected_files = staged 全集 4 项（含本 spec）。
2. 预算：M 文件 numstat 合计约 +250/-100；无新文件。
3. findings 留 1 条：词元统计为 EmptyState 占位（无数据源）；未来接
   bot_chat 返回的 usage 字段或审计日志聚合时启用。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U8-SIDEBAR",
  "family": "ui-redesign",
  "expected_files": [
    "docs/batches/U8-SIDEBAR.spec.md",
    "src/App.test.tsx",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 380,
  "max_lines_removed": 140,
  "max_new_files_lines": 100,
  "findings": [
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "「词元统计」为 EmptyState 诚实占位：全仓无 token 用量数据源；未来接 bot_chat usage 字段或审计聚合时启用" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
npx --no-install vitest --run          # 354 绿
npx --no-install tsc --noEmit -p tsconfig.json
npx --no-install oxlint src            # 0 warn / 0 error
npx --no-install knip --no-progress    # exit 0
```
