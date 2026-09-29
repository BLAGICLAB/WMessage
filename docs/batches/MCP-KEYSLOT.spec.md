# Batch Spec: MCP-KEYSLOT

## 目的

B4-6（拍板③=迁）：MCP env/headers 明文存 bot-config.json → 迁系统凭据存储。
设计 `docs/MCP-KEYSLOT-MIGRATION-DESIGN-2026-09-29.md`（A1 全量迁，2026-09-29 拍板）。
三行版：① `env`/`headers` 加 `#[serde(skip_serializing)]`（任何写路径 fail-closed，
与 tavily_key 同款）；② 新 `bot/mcp/secrets.rs`（keyring `mcp:<id>` 条目 / Linux
降级单文件 0600 / 进程内缓存 / 2048 字节上限）；③ 迁移挂 CONFIG_WRITE_LOCK 写
路径 + 启动一次（备份 → 逐台写+读回比对 → 全成才写回剥离；失败明文原样重试）。

## 修法（按层）

1. **config.rs**：字段 `skip_serializing` + 注释改写（模块头「敏感边界」声明作废
   指向 secrets.rs）+ `has_inline_secrets` 迁移判据。
2. **secrets.rs（新）**：`McpSecrets` blob（serde/上限）；`_at` 后端注入内核
  （System=keyring 条目写后读回比对；PlaintextFile=单文件 0600 tmp+rename，
   损坏容忍）；`hydrate_mcp_servers`（load_config 读路径水合，缓存命中不敲
   keychain，读失败留空不炸加载）；`store/purge/has`。
3. **io.rs**：`load_config` 水合；`migrate_mcp_server_secrets_locked`（必须持
   CONFIG_WRITE_LOCK；备份 `bot-config.backup-mcp-keys.json` 0600 一次性 →
   全成写回剥离，审计 mcp.secret_backup_created/migrated/migrate_failed）；
   **4 个锁内写点 + mcp with_locked_config 全部挂迁移**（任何配置写都会剥离
   明文，必须先迁——非 MCP 写路径漏挂 = 静默丢机密）。
4. **commands.rs**：save 先落 blob（写败整体报错配置不动；空=清条目）；delete
   成功后 purge（清败 WARN 留孤儿可追溯）。
5. **lib.rs**：启动迁移一次（持锁，与 legacy/search key 迁移同块）。
6. **前端**：SaveConfirmDialog 文案（机密存系统钥匙串）；payload/表单零改动。

## 红线

- 剥离明文的唯一途径 = keyring 已确认写入后的写回（skip_serializing 收口）。
- hydrate 读失败绝不炸配置加载（服务器连接失败可见即可）。
- kill…（无）——kill_switch 不涉机密路径。

## 测试

- 新增 6 测：blob serde 往返 / 2048 上限 / has_inline_secrets /
  skip_serializing 永不泄值（序列化输出 grep 无值 + 老配置读回认 env）/
  降级后端写读删 0600 往返 / 损坏文件 Err 不炸。
- 回归：mcp 42 / config 74 / evolution 283 / keyring 5 / vitest 38 / tsc 全绿。

## spec 起草后自查三条

1. expected_files 10【校正 ×1：补 docs/rust-bot-architecture.md——模块地图
   门禁要求新文件 secrets.rs 登记进模块树】。
2. budget：修改 +170/-15；新文件 ~460 行。
3. assertions_min 按 gate 正则填实测-1。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "MCP-KEYSLOT",
  "family": "mcp-integration",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/MCP-KEYSLOT.spec.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/bot/config/io.rs",
    "src-tauri/src/bot/mcp/commands.rs",
    "src-tauri/src/bot/mcp/config.rs",
    "src-tauri/src/bot/mcp/mod.rs",
    "src-tauri/src/bot/mcp/secrets.rs",
    "src-tauri/src/lib.rs",
    "src/components/SettingsPage/McpPanel.tsx"
  ],
  "max_lines_added": 770,
  "max_lines_removed": 30,
  "findings": [
    {"id": "B4-6", "file": "src-tauri/src/bot/mcp/secrets.rs", "line": 1, "fix": "机密 blob 存储：keyring mcp:<id> / 降级文件 0600 / 缓存 / 2048 上限 / 后端注入内核"},
    {"id": "B4-6", "file": "src-tauri/src/bot/mcp/config.rs", "line": 98, "fix": "env/headers skip_serializing fail-closed + has_inline_secrets 判据 + 模块头改写"},
    {"id": "B4-6", "file": "src-tauri/src/bot/config/io.rs", "line": 158, "fix": "load_config 水合 + migrate_mcp_server_secrets_locked（备份/读回比对/全成才写回）挂 5 个锁内写点"},
    {"id": "B4-6", "file": "src-tauri/src/bot/mcp/commands.rs", "line": 80, "fix": "save 先落 blob（写败不动配置）/ delete 后 purge（清败 WARN）"},
    {"id": "B4-6", "file": "src-tauri/src/lib.rs", "line": 303, "fix": "启动迁移一次（持锁）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/mcp/secrets.rs": 13,
    "src-tauri/src/bot/config/io.rs": 20
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 900,
    "expected_max_comments": 40
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```
cargo test mcp && cargo test config && cargo test evolution:: && cargo test keyring
npx tsc --noEmit && npx vitest run src/components/SettingsPage
```
