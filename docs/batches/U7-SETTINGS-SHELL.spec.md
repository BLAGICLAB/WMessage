# Batch Spec: U7-SETTINGS-SHELL

## 目的

老板需求：设置页比较乱，参照截图重新设计——左侧分类导航 + 右侧大标题卡片流，
点设置进入新页面、点「返回」回到进入前视图。

1. **设置壳**（SettingsPage 布局层改造，面板内容零改动）：
   - 左侧栏 w-52：「← 返回」钮（ArrowLeft，onBack 未传时 disabled）+ 四分类
     （通用 / 任务与工作区 / 机器人 / 智能体与扩展；lucide 图标，选中 nm-inset +
     aria-current）——分组顺序 = 源码面板顺序；
   - 右侧内容区：分类大标题（text-xl）+ max-w-3xl 收行宽 + 卡片流（面板 JSX
     原样保留在 section 内）；
   - 分类映射：通用 = 个人资料+通用设置；任务与工作区 = 任务数据+工作区管理；
     机器人 = 机器人设置大卡（API/模型/推理/授权/外部 API）；智能体与扩展 =
     技能+MCP+记忆演化+迁移。
2. **惰性挂载**（ocr HIGH 采纳）：分类首次激活才挂载、挂后保留（hidden 切换）
   ——首屏不跑未访问面板的加载 invoke（MigrationPanel/mcp_status/skills_list
   等按需拉取），未保存输入跨分类保留。旧单页为全面板同时挂载，此处为体验升级。
3. **App 全屏接管**：settings 视图替换左导航+主内容（早退分支），壳内自带
   CommandPalette（⌘K 不受视图影响）；`lastViewRef` 记录进入前视图，「返回」
   跳回；view state 类型复用 `RailView | "settings"`。
4. 测试同步（语义不变）：SettingsPage.test 12 处按分类导航（lazy+hidden 下
   getByRole 按 a11y 树过滤，需先点侧栏——模拟真实用户路径）；「初始渲染」
   用例改为逐分类断言；App.test 导入用例导航「任务与工作区」+ 返回钮回看板。

## 红线核对

- 面板内容 JSX 零改动（仅包裹 section/hidden/mounted 判断）；各面板 state、
  invoke 面、保存逻辑不动；主题机制/双窗口零改动。
- 全部测试 **354 绿**（用例数不变）；oxlint 0 warn；knip 0；tsc 过；perf 41×
  波动带内。

## 验收记录（视觉）

dev shim（stub invoke 罐装数据 + 真实 App）：深色「通用」分类（侧栏返回+四项、
大标题、个人资料/通用设置卡片流）、深色「机器人」分类（选中态迁移、机器人设置
大卡含模型行）、浅色「通用」——三张实拍对齐截图骨架（`gui-test-screenshots/
u7_*.png`，不入库）；返回钮点击跳回看板实测（backToBoard=true）。

## ocr review 复审处置记录

6 条（1H/1M/4L）：**HIGH** keep-mounted 全面板挂载期 effect 无条件触发——
采纳为**惰性挂载**（首次激活才挂、挂后保留；并注明旧单页本就全面板同挂，
此为体验升级非回归）；**medium** onBack 可选语义与按钮不符——补
`disabled={!onBack}` + JSDoc 对齐；low 修 1（view union 复用 RailView），
记 3（activeMeta find 微开销、RailButton rounded 重复为 U2 既有样式、
section 缩进风格）。

## spec 起草后自查三条

1. expected_files = staged 全集 5 项（含本 spec）。
2. 预算：M 文件 numstat 合计约 +220/-40；无新源文件（本 spec 新文件计入
   new_files 预算）。
3. findings 留 1 条：非激活分类 display:none——getByRole 查不到（a11y 树
   过滤）属预期行为；如未来需要全面板可发现，可改 lazy 渲染+激活聚焦管理
   （Aria accordion 模式），登记。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U7-SETTINGS-SHELL",
  "family": "ui-redesign",
  "expected_files": [
    "docs/batches/U7-SETTINGS-SHELL.spec.md",
    "src/App.test.tsx",
    "src/App.tsx",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 320,
  "max_lines_removed": 70,
  "max_new_files_lines": 100,
  "findings": [
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "非激活分类 display:none：getByRole 按可访问性树过滤属预期；如需全面板可发现可改 accordion 聚焦模式，登记" }
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
