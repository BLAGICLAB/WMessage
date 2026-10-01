# Batch Spec: U10-VENDOR

## 目的

老板需求：模型设置不按 OpenAI/Anthropic 兼容分类，直接**按厂商分类**；每个
厂商设置页含 Base URL / API 格式 / 模型列表（按截图 MiniMax 厂商页骨架）。

1. **Rust**（单字段扩展，serde 向后兼容）：`ModelEntry` + `vendor: Option<String>`
   （`#[serde(default, skip_serializing_if = "Option::is_none")]`——老配置缺字段
   加载为 None 不拒绝，None 不落盘保持文件干净）。聊天循环零改动：
   bot_get_config View 已把 active 模型摊平为 base_url/model，厂商字段仅随
   modelsByProvider 透传给前端分组用。定向测试：vendor 往返 + 老配置兼容
   （mod.rs tests +1）。
2. **前端厂商中心**（SettingsPage model 分类整段重写）：
   - 左栏厂商列表：跨双协议列表按 `entry.vendor` 聚合（老条目无 vendor → 按
     所在列表协议名兜底「OpenAI 兼容/Anthropic 兼容」），绿点 = 含全局 active；
     底部「＋ 添加厂商」开预设网格；
   - 右厂商页：标标题行（图标+名称+「设为当前使用」+删除厂商）+ Base URL
     （厂商条目共用，编辑同步）+ API 格式双钮（切换=整厂商条目在两协议列表间
     搬移，active 指针随条目走）+ API Key（全局共用，注明）+ 模型列表
     （ModelRow 原样，厂商内新增首条自动 active）+ max_tokens（anthropic）+
     保存配置；
   - **全部 handler 走 setConfig 函数式**（updater 内读最新 c.modelsByProvider，
     ocr 2 critical + 3 high stale-closure 家族根修）；`protocolOfVendorIn`
     纯函数供 updater 内查协议；
   - 预设网格改「添加厂商」语义：点选 = 新增该厂商（同名覆盖其条目）+
     setActiveVendor 直接进入厂商页 + 落盘（skipReload 防旧 mock 覆盖）。
3. 测试同步：模型 5 用例迁移到厂商流（预设选择/厂商页 Base URL+API 格式/
   添加模型不抢 active/保存透传含 vendor 字段）。

## 红线核对

- 聊天循环（bot_model_loop/bot_chat）零改动——bot_get_config View 摊平语义
  不变，active 模型的 base_url/model 解析路径与改造前一致。
- 双窗口/主题机制/看板拖拽零改动。rust 定向：config 模块 46 测试全绿；
  **test-all 全量含 Rust+审计 exit 0（150s）**；前端 354 绿 / oxlint 0 /
  knip 0 / tsc 过。

## 验收记录（视觉）

dev shim 实拍（`gui-test-screenshots/u10_*.png`）：深色厂商列表（MiniMax/
OpenAI 绿点）→ MiniMax 厂商页全要素（图标标题/当前使用 ✓/删除钮/Base URL/
API 格式双钮 Anthropic Messages 选中/API Key/模型列表 M3 active + M2.7/
max_tokens/保存配置）——对齐截图。

## ocr review 复审处置记录

29 条（2C/4H/8M/10L/5 空级别，含跨轮去重）：

- **critical 2 + high 3（同源 stale-closure 家族）全修**：厂商 handlers 从
  外层 render 闭包读 config 快照，连续操作互相覆盖 + 厂商页 ModelRow 走全局
  apiProvider 键写错列表——整块重写为 setConfig 函数式（updater 内部读最新
  state）+ `protocolOfVendorIn(cfg, name)` 纯函数 + vendor 版 handler。
- **medium 采纳**：删除厂商补 confirm + active 兜底；技能路由 configBusy
  （U8 已修，复审确认）；`void` 死代码清理。
- **medium 记录/驳回**：saveConfig 吞错不重抛（U5 既有全局行为，非本批）；
  vendorBaseUrl 以首条为准（编辑即同步全部，差异只在历史脏数据）；老条目
  兜底名与预设名撞名（概率低）；刷新浅合并（显式刷新语义，U9 已注明）。
- **low 10**：记录（缩进/命名/微冗余）。

## spec 起草后自查三条

1. expected_files = staged 全集 7 项（含本 spec）。
2. 预算：numstat 合计约 +470/-200（Rust +31 全为字段/测试；前端主体为
   SettingsPage 厂商中心整段）。
3. findings 留 1 条：API Key 仍为全局一份（keyring 单 key），厂商级 key
   需 keyring 多 slot 扩展，登记挂账。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U10-VENDOR",
  "family": "ui-redesign",
  "expected_files": [
    "docs/batches/U10-VENDOR.spec.md",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/config/schema.rs",
    "src-tauri/src/bot/config/types.rs",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/SettingsPage/types.ts"
  ],
  "max_lines_added": 560,
  "max_lines_removed": 260,
  "max_new_files_lines": 110,
  "findings": [
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "API Key 仍为全局一份（keyring 单 key）；厂商级 key 需 keyring 多 slot 扩展，登记挂账" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
bash scripts/test-all.sh               # exit 0（含 Rust + 审计）
cd src-tauri && cargo test --lib bot::config   # 46 绿（含 vendor serde 测试）
npx --no-install tsc --noEmit -p tsconfig.json
npx --no-install oxlint src            # 0 warn / 0 error
npx --no-install knip --no-progress    # exit 0
```
