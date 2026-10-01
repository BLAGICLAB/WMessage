# Batch Spec: U9-MODELHUB

## 目的

老板需求（两张截图）：「模型设置」按图一重排为模型中心，点「添加供应商」按
图二出供应商选择网格。

1. **页首行**：说明「管理模型供应商，配置后可在挂件聊天时选择使用。」+
   刷新钮（重新拉 bot_get_config，显式刷新=以服务端为准覆盖未保存改动，
   注释与 title 注明；失败 console.error 留痕）+「＋ 添加供应商」钮。
2. **双栏大卡**（对齐图一）：左栏供应商列表 = 双协议槽（OpenAI 兼容 /
   Anthropic 兼容，绿点 = 该协议已设 active 模型，选中 nm-inset +
   aria-pressed，点击切协议——替代原 ApiProviderSelect 下拉）；右栏详情 =
   模型列表（ModelRow 原样：radio active/baseUrl/model/删除）+ 添加大模型 +
   max_tokens（anthropic）+ API Key（保存/清除）+ 保存配置。
   **Base URL 维持每模型一行**（现状数据模型），不造供应商级假字段。
3. **添加供应商网格**（对齐图二，右栏子视图）：「← 返回 添加供应商」+
   覆盖提示 + 八家预设卡（DeepSeek/Kimi/MiniMax/OpenRouter/阿里云百炼/
   OpenAI/Anthropic/xAI：emoji 图标+名称+协议标注）+「创建自定义供应商」
   （关网格并自动加一行空模型）。**点预设 = 切协议 + 覆盖该协议 baseUrl/
   模型列表（预填）+ 首个设 active + 立即落盘 saveConfig**（ocr HIGH 采纳：
   原实现只 setConfig 不落盘，已修）。
4. **数据模型边界（诚实分层）**：现有后端仅双协议，同一协议仅一份配置——
   「任意多供应商并存」需 Rust config 扩展，登记挂账不做；预设 = 落在
   双协议内的整表预填（网格页注明覆盖语义）。ApiProviderSelect 组件保留
   （其独立测试仍覆盖），SettingsPage 不再引用。
5. 测试同步：切协议断言自绘下拉 option → 左栏供应商按钮（同名）。

## 红线核对

- 保存链路/state/invoke 面零改动（applyProviderPreset 落盘走既有 saveConfig）；
  双窗口/主题机制/看板拖拽零改动；src-tauri 零改动。
- 全部测试 **354 绿**；oxlint 0 warn；knip 0；tsc 过；perf 波动带内。

## 验收记录（视觉）

dev shim 实拍（`gui-test-screenshots/u9_*.png`）：深色模型中心双栏（左栏
绿点/灰点供应商、右栏两行模型+Key+保存）、添加供应商网格（八家预设卡 +
创建自定义）、DeepSeek 预设应用实测（列表替换为 deepseek-chat/reasoner +
baseUrl 预填 + 首个 active）。

## ocr review 复审处置记录

7 条（2H/4M/1L）：**HIGH 2 修**——applyProviderPreset 只 setConfig 不落盘
（补 saveConfig，对齐 toggleTavily 模式）；「创建自定义供应商」无行为
（关网格+自动加一行空模型）。**medium 3 修/记**：刷新吞错补 console.error
（显式刷新覆盖未保存改动=预期语义，注释+title 注明，记录）；预设硬编码
本文件（本批引入即此文件，MiniMax anthropic 端点按截图 API 格式，用户可改，
记录）；覆盖未确认（网格页顶已有覆盖语义提示行，记录）。low 1 记。

## spec 起草后自查三条

1. expected_files = staged 全集 4 项（含本 spec）。
2. 预算：M 文件 numstat 合计约 +220/-20；无新文件。
3. findings 留 1 条：「任意多供应商并存」需 Rust config 扩展（modelsByProvider
   泛化 + 聊天链路按供应商解析），登记挂账；本批预设落在双协议内。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U9-MODELHUB",
  "family": "ui-redesign",
  "expected_files": [
    "docs/batches/U9-MODELHUB.spec.md",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 300,
  "max_lines_removed": 60,
  "max_new_files_lines": 100,
  "findings": [
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "任意多供应商并存需 Rust config 扩展（modelsByProvider 泛化+聊天链路解析），登记挂账；本批预设=双协议内整表预填" }
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
