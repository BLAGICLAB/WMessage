# BT-03a spec — C5-BT-03 bot-config.json 原子写 + 写互斥（io.rs/schema.rs 4 条；commands.rs:86 转 B 类）

## 目标 findings（C5-BT-03 五取四）

1. **io.rs:100**（high）：write_bot_config_file 用 `std::fs::write` 原地 truncate——崩溃/断电
   留空文件 → load_config 静默回默认，allowedDirs/搜索设置全丢。
2. **io.rs:76**（high）：add_allowed_dir RMW 无锁——并发 update_config_file / migrate_* /
   第二个 add_allowed_dir 互相覆盖（finding 原文点名这些并发方）。
3. **schema.rs:170**（high）：migrate_config_file 同形态非原子写（文件可能仍含明文 apiKey，
   半截写 = 配置+密钥同毁）。
4. **schema.rs:178**（high）：SCHEMA_MIGRATION_CHECKED check-then-act——窗口内两个线程同进
   migrate_config_file 重复 RMW。

**commands.rs:86（keyring 先于文件写，无回滚）→ 转 B 类攒批**：先后调换是半成功陷阱方向
（文件先写=keyring 失败则文件引用不存在的 key；keyring 先写=文件失败则 keyring 与文件
不一致），属"协议/语义变更方向"决策，不自决。

## 修法（一套设施全覆盖）

### 设施 A：`CONFIG_WRITE_LOCK`（io.rs 新增 static）

- `pub(crate) static CONFIG_WRITE_LOCK: Mutex<()>` + `lock_config_write()` guard 函数
  （C3-1 poison 形态，eprintln + into_inner，同 db::lock_db_write 先例）。
- **全部 5 条写路径必须同持一把锁**——只锁部分写者 = 竞态残留 = 半修：
  add_allowed_dir / update_config_file / write_bot_config_file / migrate_legacy_key /
  migrate_search_keys（io.rs）+ migrate_config_file / migrate_bot_config_schema（schema.rs）。
- 锁序声明：CONFIG_WRITE_LOCK 内只可能再取 BOT_LOG_LOCK（audit_event!），反向不存在
  （audit 路径不写 config），叶锁无死锁环。keyring IO 在锁内（启动期一次性迁移，可接受）。
- **防自锁**：update_config_file 与 bot_set_config 都落到写盘内核——拆
  `write_bot_config_file_locked`（无锁内核：剥 key + 警告 + 原子写）与
  `write_bot_config_file`（lock + 内核）；schema.rs 同拆
  `migrate_config_file`（lock + 内核）/ `migrate_config_file_locked`。

### 设施 B：`write_config_atomic`（io.rs 新增）

- tmp（固定 `{name}.tmp`，锁内唯一）+ `sync_all()` fsync + `rename` + 失败清 tmp。
- 不复用 db::atomic_write（无 fsync；schema.rs:170 finding 明确要求 fsync）。
- 5 条写路径的 `std::fs::write` 全部替换。

### schema.rs:178 check-then-act

`migrate_bot_config_schema`：锁外快路径 `CHECKED.load` → 锁内**复查** →
`migrate_config_file_locked` → `store(true)`。双检形态消除窗口。

### 测试

io.rs 新增 `#[cfg(test)]`：write_config_atomic 写读一致 + rename 后无 tmp 残渣 +
覆盖写正常（3 断言）。schema.rs 不加（无既有测试模块，迁移逻辑已有集成覆盖——
tests/ 下如有）。两文件当前 assert 数均为 0。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot/config/io.rs` + `src-tauri/src/bot/config/schema.rs`。
   无签名 ripple（migrate_config_file / write_bot_config_file 对外签名不变；
   *_locked 为新增 pub(crate) 内核）。✓
2. budget → B 类：设施 A+B ≈+45；io.rs 5 写点替换 ≈+25/-15；schema.rs 双检 + 拆内核
   ≈+20/-8；测试 +20。合计 ≈+110/-23 → budget +120/-30。✓
3. findings fix 字段列 ripple → write_bot_config_file 签名不变（commands.rs 调用点不动）；
   migrate_config_file 签名不变（schema.rs 内 + tests 调用点不动）；新增 static/helper
   无 ripple。✓

自查未过不许发审。补完再审。

## 自主执行规则

spec 被 reviewer 批准后:
- agent 全权执行至 commit,不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报: hash + 批定性 + OCR 轮次 + comments 处置 + follow-up
- 过程证据落 ~/.openclaw/cache/<batch-id>/,不主动读回

## Stop 条件(触发即停,报 reviewer)

- compile_failure (无 X 形态退路)
- architecture_blocker (签名/调用点超预算)
- family_heterogeneity
- new_high_different_root (OCR r1 出的 high 非本批根因)
- gate_fail (batch-verify FAIL 且非 spec 声明调整可解)

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "BT-03a",
  "family": "non-atomic-write-rmw-race",
  "expected_files": [
    "src-tauri/src/bot/config/io.rs",
    "src-tauri/src/bot/config/schema.rs"
  ],
  "max_lines_added": 120,
  "max_lines_removed": 30,
  "findings": [
    {"id": "C5-BT-03.1", "file": "src-tauri/src/bot/config/io.rs", "line": 100, "fix": "write_bot_config_file 的 std::fs::write → write_config_atomic（tmp+fsync+rename+失败清 tmp）；拆 write_bot_config_file_locked 无锁内核防 update_config_file 自锁。ripple：无（签名不变）"},
    {"id": "C5-BT-03.2", "file": "src-tauri/src/bot/config/io.rs", "line": 76, "fix": "新增 CONFIG_WRITE_LOCK + lock_config_write()（C3-1 形态）；add_allowed_dir / update_config_file / write_bot_config_file / migrate_legacy_key / migrate_search_keys 五条写路径全持锁（只锁部分=半修）。ripple：无"},
    {"id": "C5-BT-03.3", "file": "src-tauri/src/bot/config/schema.rs", "line": 170, "fix": "migrate_config_file 的 std::fs::write → write_config_atomic；拆 migrate_config_file_locked 防 migrate_bot_config_schema 自锁。ripple：无（签名不变）"},
    {"id": "C5-BT-03.4", "file": "src-tauri/src/bot/config/schema.rs", "line": 178, "fix": "SCHEMA_MIGRATION_CHECKED check-then-act → 锁外快路径 + 锁内复查双检（锁=CONFIG_WRITE_LOCK）。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/config/io.rs": 3,
    "src-tauri/src/bot/config/schema.rs": 0
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  },
  "stop_conditions": [
    "compile_failure",
    "architecture_blocker",
    "family_heterogeneity",
    "new_high_different_root",
    "gate_fail"
  ]
}
```

## 提交信息骨架

```
fix(bot): BT-03a — bot-config.json 原子写 + 全写路径互斥（C5-BT-03 4 条）

【family】non-atomic-write-rmw-race
【实修 4 条】
- io.rs:100 write_bot_config_file std::fs::write → write_config_atomic（tmp+fsync+rename）
- io.rs:76 add_allowed_dir RMW 无锁 → CONFIG_WRITE_LOCK 全覆盖 5 条写路径
  （只锁部分写者=竞态残留=半修）；拆 *_locked 无锁内核防自锁
- schema.rs:170 migrate_config_file 同换原子写
- schema.rs:178 SCHEMA_MIGRATION_CHECKED check-then-act → 锁外快路径+锁内复查双检
【行为变更】崩溃/断电不再留空配置文件；并发写不再互相覆盖；配置写串行化
（启动期一次性迁移含 keyring IO 在锁内，可接受）。
【D2】行为断言：write_config_atomic 写读一致+无 tmp 残渣+覆盖写 3 断言。
前置断言：序列化格式（to_string_pretty）与 key 剥离逻辑逐字保留。反例断言：
若退回 std::fs::write，tmp 残渣用例失去意义。
【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/BT-03a.spec.md python3 scripts/batch-verify.py docs/batches/BT-03a.spec.md
```

## 不在本批

- **commands.rs:86**（keyring 先于文件写无回滚）→ 转 B 类攒批（半成功陷阱方向决策）。
- db::atomic_write 不加 fsync（通用 JSON 落盘现状，改它影响面另评）。
- bot-config.json 文件权限 0644 → 0600（明文 key 残留期的纵深防御，finding 未要求，另评）。
