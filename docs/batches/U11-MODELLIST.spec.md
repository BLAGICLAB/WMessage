# Batch Spec: U11-MODELLIST

## 目的

老板需求（截图）：厂商设置页模型列表改为紧凑行（模型名 mono + 上下文徽标 +
编辑铅笔 + 启用开关）；模型列表可从厂商网站获取；设置页不再显示推理强度
（只保留聊天窗内设置）。

1. **Rust**：ModelEntry + `enabled: bool`（serde default true——老配置缺字段
   默认启用）+ `context_k: Option<f64>`（上下文千数，徽标显示「204.8K」；
   None 不落盘）。ModelsByProvider 去 Eq（f64）。新命令
   `fetch_provider_models(base_url, api_format, api_key)`：GET {base_url}/models，
   OpenAI 格式带 Bearer / Anthropic 格式带 x-api-key（key 空则省略鉴权头），
   请求级 20s 超时 + 响应 1MB 上限，解析 `data[].id`（解析失败报错而非折叠成空，
   可区分「厂商返回 0 个」与「响应坏」）。注册 lib.rs。纯函数 parse_models_json
   + 单测（OpenAI/Anthropic 形状 + 坏 JSON）。
2. **ModelRow 紧凑行重写**（对齐截图）：非编辑态 = radio（role=radio +
   aria-checked，设为当前）+ 模型名 mono 文本 + contextK 徽标 + 铅笔（进编辑）
   + Toggle 启用开关（复用 Toggle 原语，自带 focus-visible；enabled=false =
   聊天 🧠 下拉过滤）；编辑态（铅笔切换）= 名称/Base URL/model 三输入 + 删除。
3. **厂商页模型列表头**：+「从厂商获取」钮 → fetch_provider_models → 合并
   （同厂商按 model id 去重，新 id append 默认禁用，其他厂商条目不动）+
   结果文案（区分「新增 N 个」与「均已存在」）→ **显式 saveConfig(next,
   {skipReload:true})**（ocr critical 修：无参 saveConfig() 读闭包旧 config
   会丢刚拉的模型）。busy/disabled 状态 + title 注明前置条件。
4. **推理强度卡移除**（model section）：后台默认字段（config.reasoningEffort、
   load 归一化、saveConfig 透传）保留——聊天 🧠 会话级覆盖照旧可用；仅设置页
   UI 入口移除（老板拍板「只在聊天窗里设置」）。setReasoningEffort 死代码删除。
5. 测试同步：模型 5 用例迁移到紧凑行交互（文本断言替代 displayValue、编辑态
   点删除、saveConfig 取最后一次 bot_set_config——预设落盘与保存按钮各一次）；
   初始渲染删推理强度断言；推理强度用例整体删除（UI 已移除）。

## 红线核对

- 聊天循环零改动（enabled/contextK 仅随透传列表走；active 模型解析路径不变）；
  ChatPanel 🧠 下拉**本批不改过滤**（enabled 过滤将随 U12 聊天批一并做，避免本批
  触碰 ChatPanel）——开关当前仅存配置语义。
- 双窗口/主题机制/看板拖拽零改动。
- 前端 353 绿（-1：推理强度用例随 UI 移除删除）/ oxlint 0 / knip 0 / tsc 过；
  cargo config 47 绿；**test-all 全量含 Rust+审计 exit 0（120s）**。

## 验收记录（视觉）

dev shim 实拍（`gui-test-screenshots/u11_modellist_dark.png`）：模型列表紧凑行
（MiniMax-M3 mono + 1024K 徽标 + 铅笔 + 红色 Toggle 启用中 / M2.7、M2.5 灰点
+ 204.8K + 铅笔 + 关）+「从厂商获取」「＋ 添加模型」双钮 + max_tokens——
对齐截图行结构。

## ocr review 复审处置记录

27 条（1C/3H/12M/9L/2 空，含跨轮去重）：

- **critical 修**：fetchModelsForVendor 的 `saveConfig()` 无参读闭包旧 config，
  新拉模型静默丢失——改为显式 `saveConfig(next, {skipReload:true})`。
- **high 3**：fetch 无请求级超时（补 20s + 1MB 响应上限）；响应解析失败折叠成
  空列表（改 Err 上抛，前端 toast）；ContextBadge 死 UI（contextK 此前无来源
  ——本批预设/获取链路开始填充，非死代码，驳回；UI 已启用）。
- **medium 修 6**：api_format String → 校验仅认两格式；空 key 省略鉴权头；
  消息区分「返回 0 个」与「均已存在」；title 补全禁用条件；注释与代码一致；
  手写开关换 Toggle 原语（focus-visible 自带）。**medium 记 2**：context_k 用
  f64（JSON 输入无 NaN，展示用可接受）；推理强度字段仍随配置透传（聊天仍消费）。
- low/空级别 11 记。

## spec 起草后自查三条

1. expected_files = staged 全集 9 项（含本 spec）。
2. 预算：numstat 合计约 +310/-170（Rust +110 全为字段/命令/测试）。
3. findings 留 1 条：厂商级 API Key 未做（keyring 单 key），获取模型列表走
   全局 key；keyring 多 slot 扩展登记挂账。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U11-MODELLIST",
  "family": "ui-redesign",
  "expected_files": [
    "docs/batches/U11-MODELLIST.spec.md",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/config/schema.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/lib.rs",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/ModelRow.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/SettingsPage/types.ts"
  ],
  "max_lines_added": 420,
  "max_lines_removed": 200,
  "max_new_files_lines": 115,
  "findings": [
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "厂商级 API Key 未做（keyring 单 key），获取模型列表走全局 key；keyring 多 slot 扩展登记挂账" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
bash scripts/test-all.sh               # exit 0（含 Rust + 审计）
cd src-tauri && cargo test --lib bot::config   # 47 绿（含 fetch parse 测试）
npx --no-install tsc --noEmit -p tsconfig.json
npx --no-install oxlint src            # 0 warn / 0 error
npx --no-install knip --no-progress    # exit 0
```
