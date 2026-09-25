# Batch Spec: APW-01

## 目的

C5-DB-02a.migrations 单条：legacy 单绑定 `file_path/file_is_dir` → 新 `files` JSON 数组的迁移函数 `migrate_legacy_file_bindings`（migrations.rs:35）当前**UPDATE loop 无事务**——逐条 `conn.execute("UPDATE tasks ...")` 在 autocommit 下逐条提交，循环中第 N 条失败时前 N-1 条已落库，DB 留半截迁移。

修法：函数签名 `&rusqlite::Connection` → `&mut rusqlite::Connection`；UPDATE 循环外层 `let tx = conn.transaction()...`；循环内 `conn.execute` → `tx.execute`；循环后 `tx.commit()...`。失败时 SQLite transaction 自动 rollback（`tx` drop 时），调用方拿到 Err，DB 状态不变。

family = **atomicity-partial-write**（新 family 首开）。

## 人类可读摘要

- family: atomicity-partial-write（首开）
- 覆盖 findings: 1（C5-DB-02a.migrations）
- 预估 diff: 2 files / +10/-0 lines（含签名 ripple，详见 expected_files 注释）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5

## 红线

- family 一致性：本批只含 atomicity-partial-write 一条 finding（其余 2 条拆 APW-02，本批不混）
- 不写"待定" / "同结构改"（按 SPEC-001 教训）
- 不引入 silent fallback / log-and-continue 形态
- 调用方 `let mut` 升级 + `&mut conn` 改动 + 定义签名改 = 6 个改动点（call site 5 + 定义 1），超出立即停手报

## Stop 条件（触发即停，报 reviewer）

- compile_failure
- architecture_blocker
- family_heterogeneity
- new_high_different_root
- gate_fail

## 机器可读（脚本读取，勿改格式）

预算修正说明：`max_lines_removed` 由 8 改 10。-8 估算错误（拍脑袋给数，未逐点算）：7 处 ripple 各 1 行 modified = -7；签名改 +1/-1；事务包裹不净增删；execute 同行 modified 不计。实测 -9，修正为 -10 留 1 行 buffer。

```json
{
  "batch_id": "APW-01",
  "family": "atomicity-partial-write",
  "expected_files": [
    "src-tauri/src/db/migrations.rs",
    "src-tauri/src/db/mod.rs"
  ],
  "max_lines_added": 15,
  "max_lines_removed": 10,
  "findings": [
    {
      "id": "C5-DB-02a.migrations",
      "file": "src-tauri/src/db/migrations.rs",
      "line": 35,
      "fix": "函数签名 `pub fn migrate_legacy_file_bindings(conn: &rusqlite::Connection) -> Result<usize, String>` 改为 `(conn: &mut rusqlite::Connection)`；UPDATE 循环 for (id, path, is_dir) in rows 体外先 `let tx = conn.transaction().map_err(|e| e.to_string())?;`；循环体内 `conn.execute(\"UPDATE tasks SET files = ?1 WHERE id = ?2\", ...)` 改 `tx.execute(...)`；Ok(n) 之前加 `tx.commit().map_err(|e| e.to_string())?;`。SQLite transaction drop 自动 rollback，调用方拿到 Err，DB 状态回到循环前。【签名 ripple 同一修复动作】mod.rs:76 `let conn = rusqlite::Connection::open(&db_path)?` → `let mut conn = ...`；mod.rs:193 `migrations::migrate_legacy_file_bindings(&conn)?` → `migrate_legacy_file_bindings(&mut conn)?`；mod.rs:583 测试同两处升级（`let conn` → `let mut conn` + `&conn` → `&mut conn`）；mod.rs:626 同；mod.rs:649 同。ripple 5 处（生产 2 + 测试 6 升级 = 5 处插入点，单点字符级 mut 插入）。无深层级联（mod.rs:76 是 open_db 函数顶层局部变量，不下传）。"
    }
  ],
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 5
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

## 签名核（执行前已验）

`src-tauri/src/db/migrations.rs:35` `pub fn migrate_legacy_file_bindings(conn: &rusqlite::Connection) -> Result<usize, String>`
  → 改 `(conn: &mut rusqlite::Connection)`

`src-tauri/src/db/migrations.rs:23-33` SELECT 语句（rows 收集）**不动**——只在事务外 SELECT，然后在事务内 UPDATE，符合 SQLite 事务语义（事务隔离级默认 DEFERRED，SELECT 不阻塞）。

**【已知窗口：本批不修】** mod.rs:193 `migrate_legacy_file_bindings` 调用点在 `open_db` 函数内，**不在 DB_WRITE_LOCK 内**（line 77 注释“进程内写者已由 DB_WRITE_LOCK 串行化”指进程级其他写者；open_db 自己运行时不持锁）。SELECT 与 UPDATE 之间存在 TOCTOU 窗口——另一写者可在这个窗口内改数据，事务只保 UPDATE 原子，不防 SELECT 快照过期。**TOCTOU 归 C5-DB-03 独立批**；本批仅修"UPDATE 原子性"一个动作。

实际场景下：open_db 启动时迁移阶段进程内无并发写者，TOCTOU 窗口为理论窗口。但 spec 不可依赖“运行时无并发”这个隐含前提——锁持有与否需在代码上显式表达。

`src-tauri/src/db/migrations.rs:58-61` UPDATE 循环：
```rust
for (id, path, is_dir) in rows {
    let files = serde_json::to_string(&vec![super::TaskFile { path, is_dir: ... }]).map_err(|e| e.to_string())?;
    conn.execute("UPDATE tasks SET files = ?1 WHERE id = ?2", rusqlite::params![files, id]).map_err(|e| e.to_string())?;
    n += 1;
}
Ok(n)
```
→ 改为：
```rust
let tx = conn.transaction().map_err(|e| e.to_string())?;
for (id, path, is_dir) in rows {
    let files = serde_json::to_string(&vec![super::TaskFile { path, is_dir: ... }]).map_err(|e| e.to_string())?;
    tx.execute("UPDATE tasks SET files = ?1 WHERE id = ?2", rusqlite::params![files, id]).map_err(|e| e.to_string())?;
    n += 1;
}
tx.commit().map_err(|e| e.to_string())?;
Ok(n)
```

## 调用点影响清单（5 处 + 1 定义 = 同一修复动作）

1. `src-tauri/src/db/mod.rs:76`（生产）：`let conn = rusqlite::Connection::open(&db_path)` → `let mut conn = ...`
2. `src-tauri/src/db/mod.rs:193`（生产）：`migrations::migrate_legacy_file_bindings(&conn)?` → `migrations::migrate_legacy_file_bindings(&mut conn)?`
3. `src-tauri/src/db/mod.rs:583`（测试）：`let conn = ...` → `let mut conn = ...`；`migrate_legacy_file_bindings(&conn)` → `(&mut conn)`
4. `src-tauri/src/db/mod.rs:626`（测试）：同上
5. `src-tauri/src/db/mod.rs:649`（测试）：同上
6. `src-tauri/src/db/migrations.rs:35`（定义）：签名改 `&Connection` → `&mut Connection`

**级联评估**：5 处 ripple + 1 定义。**无深层级联**（mod.rs:76 是 `open_db` 函数顶层局部变量，函数返回时不传出 conn 给其他函数）。全部在本批。

## 提交信息骨架

```
fix(db): APW-01 — legacy 单绑定迁移包事务（atomicity-partial-write family 首开）

【family】atomicity-partial-write（新 family 首开）
C5-DB-02a.migrations：legacy file_path/file_is_dir → files JSON 迁移 UPDATE loop 逐条
conn.execute 在 autocommit 下提交，第 N 条失败时前 N-1 条已落库 → DB 留半截迁移。

【实修】（2 文件，签名 ripple 是同一修复动作）
- db/migrations.rs:35（finding 原发）：函数签名 &Connection → &mut Connection；UPDATE loop
  外包 `let tx = conn.transaction()?`；循环内 conn.execute 改 tx.execute；循环后 tx.commit()?。
  SQLite transaction drop 自动 rollback，调用方拿到 Err，DB 状态不变。
- db/mod.rs（签名 ripple，5 处同一修复动作）：
  · line 76 生产 `let conn` → `let mut conn`
  · line 193 生产 `migrate_legacy_file_bindings(&conn)?` → `(&mut conn)?`
  · line 583 测试同两处升级
  · line 626 测试同两处升级
  · line 649 测试同两处升级
  无深层级联（mod.rs:76 是 open_db 函数顶层局部）。

【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
【OCR】r1：期望 ≤ 5 comments（2 文件改动 + 签名 ripple 是 OCR 常见"你想清楚了吗"触发点）

【family】atomicity-partial-write（成员 +1：C5-DB-02a.migrations）
【拆出】C5-DB-02a.paths + C5-MI-06 拆 APW-02，复杂度差一个量级不同批
```

## 新立原则（2026-09-23 拍）—— 写给未来 spec 起草

1. **`expected_files` 覆盖全部写入路径**——包括签名级联、调用点 ripple、文件生成物。trigger 是"改动动作"，不是"finding 原发地"。expected_files = 本批 `git add` 的全部文件，不只是 finding 所在的文件。语义：文件集是写入边界，不写 = 不动 = 不在批 scope 内。

2. **findings 逐条 fix 字段若触发其他文件改动，显式列出**——在被触达的文件 + 行号层面（不是描述层）。fix 字段是脚本读取的事实声明，"同一修复动作"必须显式归类，不能用"wontfix-with-rationale"事后擦（OCR 评论须在 finding 范围内处理，不在 gate 范围软化）。

→ 推论：gate 的 `file_set` "exact match" 卡是**真检查**，不是"声明后用 wontfix 绕"。expected_files 错 = spec 错 = 重写，不是 spec 对 + OCR 不吵。

→ 待落点（提案，非本批动作）：`docs/BATCH-SPEC-TEMPLATE.md` § "expected_files" 段加入这两条；PHASE2-TRIAGE.md §5 SOP 加入"spec 起草前置核——expected_files = 写入路径全集 + fix 字段尾列 ripple"。

## budget 计算规则（2026-09-23 拍，长期规则）

```
max_lines_added/removed = Σ(改动点 × 每点行数)
- 签名改: +1/-1 per 定义 + 每 call site +1/-1
- 事务包裹: +2/-0 (let tx + commit)
- execute 替换: +0/-0 (同行 modified)
- 严禁凭印象给整数
```

教训：APW-01 原估 -8 未逐点算，gate 跑出 -9 FAIL。budget 是事实的一部分（与签名核、调用链核并列），不许凭印象。

→ 推论：下次 spec 起草，budget 与签名核/调用链核同走三检：每点改动算 +/−,sum 后 ± 1 行 buffer（不留大余量）。

→ 待落点（提案，非本批动作）：`docs/BATCH-SPEC-TEMPLATE.md` § "max_lines_added/removed" 段加入此计算规则；本批 commit message 提一句"按此规则重算 budget"。

## 验证命令

```bash
BATCH_SPEC=docs/batches/APW-01.spec.md python3 scripts/batch-verify.py docs/batches/APW-01.spec.md
```

## 不在本批

- C5-MI-06 copy_dir_recursive（FS staging + rename，中等复杂度）→ APW-02
- C5-DB-02a.paths copy_legacy_db（3 文件 staged swap + rollback，高复杂度，路径草案待重写）→ APW-02
- atomicity-partial-write family 后续 batch（C5-DB-02b 等）→ 待 APW-01 收口后开