# Batch Spec: U13-MODELPARAMS

## 目的

每模型推理参数接线 + 遗留收尾（U12 挂账兑现）。ModelEntry 的
temperature/top_p/max_tokens/system_prompt 自 U11 起已持久化、前端可编辑，
但三处请求装配（聊天主循环 / 摘要 / Planner）只读全局值——用户填了没反应。
本批：后端新增 `active_model_entry`（当前协议 + active_model_id → active 条目，
解析与 `derive_legacy_fields_from_active` 同源：active id 命中优先，悬空/槽位
未选回退第一条，拆参签名同 `active_vendor_of`）+ `effective_inference`（条目值
> 全局值 > 内置默认）；`LlmHttp` 扩 temperature/top_p/system_prompt，纯函数
`apply_inference_params` 注入请求体（OpenAI 与 Anthropic /v1/messages 顶层
字段同名，一处实现两协议共用）。max_tokens 条目值覆盖全局后走 8192 兜底与
256..=200000 钳制，且同一有效值喂给 reasoning::resolve 的 thinking budget
夹紧；OpenAI 格式维持不发送。条目 system_prompt 追加聊天主循环消息栈末尾
（摘要/Planner 有各自固定任务提示词，不追加）。收尾：ModelRow 编辑态说明
「max_tokens 仅 Anthropic 格式生效」；模型设置页首在「有带 vendor 条目但
verifiedVendors 为空」时显示升级引导（无自动迁移）；applyVendorModels 用
normalizeVendorName 查重合并，修掉预设中文名 vs 模型库英文名造成的同厂商
两条目并存（合并保留既有厂商名——keyring vendor:{名} 条目与 verified_vendors
按既有名登记，换名即丢 key）。

## ocr 复审处置记录（提交前全量 diff 审查，4M/12L，json 于 docs/OCR-CODE-REVIEW-2026-10-03-u13.json）

- **M 修 1（两条同源）**：temperature/top_p 无区间校验——条目值自由输入，
  Anthropic 对 temperature>1.0 直接 400，一次超界采样参数毒断整轮对话。
  按 resolve_max_tokens 先例在 schema.rs 加 `resolve_temperature`（协议感知：
  Anthropic 0..=1 / OpenAI 兼容 0..=2）与 `resolve_top_p`（0..=1），
  effective_inference 统一出口钳制；补钳制矩阵单测。
- **M 拍板保留 1**：条目 system_prompt 追加在消息栈末尾、跨轮持久——
  「追加到消息栈末尾」是本批 spec 明文语义（老板拍板），不改位置；
  OpenAI 兼容网关对尾部 system 消息的位置容忍度差异登记（Anthropic 侧
  转换时归并进顶层 system，无此问题）。
- **L 修 5**：EffectiveInference 补 `#[derive(Debug, Clone)]`（同 config
  结构体惯例）；system_prompt trim 先行（免空白串白分配一次 + 保留 trim 后
  值，doc 语义对齐）；`__legacy__` 哨兵与 vendorNameOf 兜底名的差异补注释；
  enabled=false 契约锁定测试（enabled 是 UI 过滤语义，后端 active 解析不受
  影响——显式决策点入测）。
- **L 登记不修 4**：apply_inference_params 双 Option 位置参数（当前仅三个
  调用点、参数名字面清晰，单测锁定注入行为；引入参数结构体收益不足）；
  anthropic 采样单测的 max_tokens 断言系「非覆盖」文档式断言（注明语义）；
  llm.request 审计不新增 temperature/top_p kv（本批边界「不加多余日志」，
  采样行为可由 bot-config.json 与请求体对账）；let-else 改写建议（现 `?`
  早退与 active_vendor_of 同风格）。
- **不适用 3 + 2**：model-meta-service/main.py 三条（[4][9][10]）——该目录
  untracked 未入库、U12-MODELGOV 已拍板由 Rust meta 模块废弃替代，不属于
  本批 staged 集合；其余 low 与上述重复合并。

## 红线核对

- 可用性门禁零改动：prune_verified_vendors 未动——条目参数不进（协议,URL）
  签名，参数变更不使 verified_vendors 失效（新增回归单测锁定）。
- key 本体永不落配置文件；keyring 读写路径零改动。
- OpenAI 格式不发送 max_tokens（维持）；Anthropic max_tokens 兜底/钳制路径
  不变，仅允许条目值覆盖。
- 摘要 / Planner 不消费条目 system_prompt（固定任务提示词不被用户人设指令
  污染）；temperature/top_p/max_tokens 三处消费点一致。
- 双窗口 / 主题 / 看板 / 工具权限 / 自进化模块零改动。

## 明确不做（承任务书）

- 孤儿 keyring 条目清理（keyring 无法枚举）。
- OpenAI 格式发 max_tokens。
- meta 同步删除上游下架项（继续登记）。
- verifiedVendors 空的自动迁移（只做页首引导，逐厂商点插头恢复）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U13-MODELPARAMS",
  "family": "backend-wiring",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U13-MODELPARAMS.spec.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/mod.rs",
    "src-tauri/src/bot/config/schema.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_plan.rs",
    "src-tauri/tests/llm_integration.rs",
    "src-tauri/tests/mock_llm.rs",
    "src-tauri/tests/task_chat_exec.rs",
    "src/components/SettingsPage.test.tsx",
    "src/components/SettingsPage/ModelRow.tsx",
    "src/components/SettingsPage/SettingsPage.tsx"
  ],
  "max_lines_added": 1050,
  "max_lines_removed": 40,
  "max_new_files_lines": 220,
  "findings": [
    { "file": "src-tauri/src/bot/config/schema.rs", "note": "temperature/top_p 无全局字段，条目值按协议钳制（Anthropic 0..=1 / OpenAI 0..=2、top_p 0..=1）后注入；enabled 为 UI 过滤语义，后端 active 解析不受影响（测试锁定）" },
    { "file": "src-tauri/src/bot_model_loop.rs", "note": "条目 system_prompt 追加消息栈末尾为 spec 明文语义；OpenAI 兼容网关对尾部 system 的位置容忍度差异登记" },
    { "file": "src/components/SettingsPage/SettingsPage.tsx", "note": "applyVendorModels 归一化查重保留既有厂商名（keyring/verified 连续性）；verifiedVendors 空引导行，无自动迁移" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
bash scripts/test-all.sh               # exit 0（nextest 1332 / pytest 审计 / vitest 全量）
cd src-tauri && cargo nextest run -E 'test(schema) or test(inference_param)'   # 条目级参数解析矩阵
npx --no-install tsc --noEmit -p tsconfig.json
```
