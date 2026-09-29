# Batch Spec: EV-B1

## 目的

按 `docs/AUDIT-FIX-PLAN-2026-09-29.md` B1 批（自进化数据安全，含 2 个 P0）：

- **B1-1（P0-EV1）** BotConfig 保真 evolution 块：未知字段曾被 serde 静默丢弃，
  设置页任意一次写盘把 `shadow.enabled / activation.mode / activation_state` 写丢
  （运行时配置上实际发生，恢复前 evolution 观察态全裸奔）。方案 A（拍板）：
  `BotConfig` 增 `evolution: Option<serde_json::Value>` 原样透传 +
  `bot_set_config`（前端整体替换写，视图不含此块）落盘前从盘上现值回填
  （盘上值权威——该块只归 evolution/ 模块写）。三条写路径
  （bot_set_config / update_config_file / persist_last_run）全覆盖回归。
  运行时 `bot-config.json` 的 evolution 块已按 OBSERVATION_STATUS §1 原值手工恢复。
- **B1-2（P0-EV2 半）** delete 级联 lesson：`cascade_source=true` 时除 related_refs
  源记忆外，同时按 `tags[0]=evo:<proposal_id>` 删 apply 落下的 lesson——
  否则废案提案的 lesson 成孤儿，injection_block 永远带出。
- **B1-3（P0-EV2 另半）** rollback 够得着自动应用：apply 落 lesson 的同时补写
  ChangeRecord（Active + AutoApplied）到 evolution-changes.jsonl——面板回滚只读
  这个文件，此前自动应用只落 applied.jsonl（面板不读），回滚对自动应用不可达。
- **B1-4** 存量孤儿清理：dev 库（target/debug/wmessage.db）2 条孤儿 lesson
  （`evo:d1b23b6e4eb80156` / `evo:48f454e7491a62bb`）按 `delete_by_key_tag`
  精确语义（tags 首元素匹配）删除，删前备份 `.bak-b1-4-20260929`；删 2 剩 0。

## 修法（按层）

1. types.rs：`BotConfig.evolution` 字段（skip_serializing_if + default；Default 补 None）。
2. config/commands.rs：`preserve_evolution`（纯函数内核，盘上值权威）+ bot_set_config 接线。
3. evolution/panel/commands.rs：`cascade_delete_mem_items`（注入连接可单测）接 delete 级联。
4. evolution/apply.rs：Applied 分支补写 CR（留痕失败仅 Warn audit 不炸 apply）。

## 红线

- 无 evolution 块的配置序列化不得引入空键（skip_serializing_if，文件 diff 干净）。
- apply 主链路失败语义不变：CR 留痕失败只 Warn，不影响 lesson 已生效的事实。
- delete 的 cascade_source 默认仍关（老板拍板不变）。

## 测试

- 新增 4 个回归：`preserve_evolution_disk_value_is_authoritative`（整体替换写路径）、
  `evolution_block_survives_config_rewrite_cycle`（serde 层 = 三条写路径公共层 + RMW 二次改写）、
  `applied_proposal_records_active_change`（CR 营盘可读回）、
  `cascade_mem_items_deletes_refs_and_lesson`（级联删 + 幂等）。
- 实测：evolution:: 277 / config 74 / mcp 36 / registry 20 / model_loop 44 全绿。

## spec 起草后自查三条

1. expected_files 8：六个源文件 + DEVLOG + 本 spec（derive.rs 为 ocr r3 HIGH②
   的 auto_applied_from_proposal 落点，二次校正补入）。
2. budget：修改 +330/-3（DEVLOG B1 条目 + ocr 处置段）；新文件仅本 spec。
3. assertions_min 按 gate 正则填实测-1（staged 全文计数）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-B1",
  "family": "evolution-data-safety",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/EV-B1.spec.md",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/io.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/change/derive.rs",
    "src-tauri/src/evolution/panel/commands.rs"
  ],
  "max_lines_added": 450,
  "max_lines_removed": 10,
  "findings": [
    {"id": "B1-1", "file": "src-tauri/src/bot/config/types.rs", "line": 168, "fix": "BotConfig.evolution Option<Value> 原样透传（P0-EV1）"},
    {"id": "B1-1", "file": "src-tauri/src/bot/config/commands.rs", "line": 99, "fix": "preserve_evolution 盘上值权威回填 + 同锁化写（评审 HIGH：TOCTOU 丢更新）"},
    {"id": "B1-1", "file": "src-tauri/src/bot/config/io.rs", "line": 495, "fix": "serde 层三写路径公共保真回归"},
    {"id": "B1-2", "file": "src-tauri/src/evolution/panel/commands.rs", "line": 205, "fix": "cascade_delete_mem_items：related_refs + evo:<id> lesson 同删"},
    {"id": "B1-3", "file": "src-tauri/src/evolution/apply.rs", "line": 196, "fix": "Applied 分支补写 Active+AutoApplied ChangeRecord"},
    {"id": "B1-3", "file": "src-tauri/src/evolution/change/derive.rs", "line": 89, "fix": "auto_applied_from_proposal：合法流转达成 Active（评审 HIGH：不裸写 status 绕状态机），生产/测试共用"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/config/commands.rs": 3,
    "src-tauri/src/bot/config/io.rs": 19,
    "src-tauri/src/evolution/apply.rs": 31,
    "src-tauri/src/evolution/change/derive.rs": 25,
    "src-tauri/src/evolution/panel/commands.rs": 22
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 900,
    "expected_max_comments": 30
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```
cargo test evolution:: && cargo test config && cargo test mcp && cargo test registry && cargo test model_loop
```
