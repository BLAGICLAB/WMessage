# Batch Spec: APW-02b

## 目的

C5-DB-02a.paths（`copy_legacy_db`，paths.rs:48）当前**3 文件 swap 沿用 Phase A + sidecar 顺序 + 边车失败仅 warn push** —— sidecar copy 失败时 db_path 已 swap 但 -wal/-shm 缺位，下游 `Connection::open` 可能成功，返回 broken DB silently（现状 bug，open_db:76 旁路 catch 不到）。

修法：3 文件 swap 改 Phase 1 (copy 3 files to staging) + Phase 2 (rename order: sidecar-first then main) + 失败全回滚（删已建 db_path-* + 清 tmp-* + 返 Err）。caller mod.rs:65 改 match Ok/Err —— Err 直接冒泡至 open_db。

open_db 之上另有 2 处 silent caller（scheduler:19 / manage:104），本次修复不穿透——归 future batch。

family = **atomicity-partial-write**（成员 +1：C5-DB-02a.paths）。与 APW-02a 同 family 但 scope 不同（FS 递归 copy vs 3 文件 swap），故拆 APW-02b（沿 APW-02a 拆批先例）。

## 人类可读摘要

- family: atomicity-partial-write
- 覆盖 findings: 1（C5-DB-02a.paths）
- 预估 diff: 2 files / +90/-30 lines（按 SOP §5 新公式 A/B 类估算，执行时按 git diff 实测校正一次）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 8

## 红线

- family 一致性：本批只含 atomicity-partial-write 一条 finding（拆 APW-02b，APW-02a 已落）
- 不写"待定" / "同结构改"（按 SPEC-001 教训）
- 不引入 silent fallback / log-and-continue 形态（函数层 + caller 层触发范围内）
- 对 race 窗口：信任 caller 前置检查 `if !db_path.exists()`，归 C5-DB-03 TOCTOU 批处理（本批不引入新防护，写明前提与归属）
- 不为上层 caller 静默吞错负责（详见 §Caller 上层 Err 处置审计）

## Stop 条件

- compile_failure / architecture_blocker / family_heterogeneity / new_high_different_root / gate_fail

## 机器可读（脚本读取，勿改格式）

预算说明：按 SOP §5 新公式（A 类行内 +1/-1，B 类结构性整块 +N/-N），估算 +90/-30。**执行时按 `git diff --cached --numstat` 实测校正一次**（按你 16:54 ④ 拍）；校正只一次，不反复调参。

```json
{
  "batch_id": "APW-02b",
  "family": "atomicity-partial-write",
  "expected_files": [
    "src-tauri/src/db/paths.rs",
    "src-tauri/src/db/mod.rs"
  ],
  "max_lines_added": 144,
  "max_lines_removed": 26,
  "findings": [
    {
      "id": "C5-DB-02a.paths",
      "file": "src-tauri/src/db/paths.rs",
      "line": 48,
      "fix": "签名 `pub fn copy_legacy_db(...) -> Vec<String>` 改 `-> Result<Vec<String>, CommandError>`（A 类行内）；函数体从 `let tmp = ...` 起到 `warns` 止（含 Phase A + sidecar for 循环）整块替换为 Phase 1 (copy 3 files to staging tmp-*) + Phase 2a (rename tmp_wal → db_path-wal) + Phase 2b (rename tmp_shm → db_path-shm，含回滚 2a) + Phase 2c (rename tmp_main → db_path，含回滚 2a+2b)（B 类结构性重写）；三失败分支统一形态：删已建 db_path-* → 清 tmp-* → 返 Err；前置 trust `if !db_path.exists()`（race 归 C5-DB-03 TOCTOU 批）；mod.rs:65 caller 改 `for w in ... { ... }` → `match { Ok(warns) => for w in ... { audit_log }, Err(e) => return Err(e.to_string()) }`（B 类整块替换，caller 层 fail-closed）；mod.rs 既有 2 测试 line 370 + 401 加 `.unwrap()` 拿 warns（A 类行内）；新增 `copy_legacy_db_returns_err_when_legacy_db_missing` 测试 + 新增 `copy_legacy_db_writes_three_files_returns_warns` 测试。【同 family 与 APW-02a 不同点】本批签名 ripple 触 caller 必改（APW-02a 无 signature ripple）；函数层 fail-closed + caller 层 fail-closed 双层（APW-02a 仅函数层）；family 双层 fail-closed 是 v4 改 caller 处置的核心理由——消除现状 sidecar 失败 silent broken DB bug。【open_db 签名仍是 Result<_, String>] caller 的 `Err(e.to_string())` 对齐；若未来 open_db 迁 `Result<_, CommandError>`，此处同步改。"
    }
  ],
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 8
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

- 公开 `copy_legacy_db(legacy_db: &Path, db_path: &Path) -> Vec<String>` → `-> Result<Vec<String>, CommandError>`（A 类行内改）
- 公开 `open_db<R>(app) -> Result<rusqlite::Connection, String>`（**保持 String**，caller 的 `Err(e.to_string())` 对齐现状签名）

## 调用点影响清单

| # | caller | 行号 | 形态 | 影响 |
|---|---|---|---|---|
| 1 | `db/mod.rs:65` open_db 内的 `for w in paths::copy_legacy_db(...) { ... }` | line 65 | 返 Err 形态变化（原 Vec<String> 兼容 for → Result 须 match 包装） | **caller 必改**：整块 B 类替换 for → match |
| 2 | `db/mod.rs:370` 测试 `let warns = copy_legacy_db(...)` | line 370 | Vec<String> 形态 → Result<Vec<String>, _> | A 类行内加 `.unwrap()` 拿 warns |
| 3 | `db/mod.rs:401` 同 #2 | line 401 | 同 #2 | A 类行内加 `.unwrap()` |
| 4 | **新增** 测试 `copy_legacy_db_returns_err_when_legacy_db_missing` | tests mod | 新增完整测试块（B 类 12 行） | 测试文件位置（紧邻既有 copy_legacy_db tests） |
| 5 | **新增** 测试 `copy_legacy_db_writes_three_files_returns_warns` | tests mod | 新增完整测试块（B 类 12 行） | 同上 |

`move_entry` 等其他 copy_legacy 函数不动。本批 only 改 paths.rs 的 `copy_legacy_db` + mod.rs 的 line 65 caller + 既有 2 测试 + 2 新测试。

## Budget 逐点算（SOP §5 新公式 A/B 类）

### A 类（行内小改：+1/-1 per 改动点）

| # | 改动点 | + | - | 备注 |
|---|---|---|---|---|
| 1 | 签名 `-> Vec<String>` → `-> Result<Vec<String>, CommandError>` | 1 | 1 | 单 token 类型变更 |
| 2 | 既有测试 line 370 + 401 `copy_legacy_db(...)` → `.unwrap()` 追加 | 2 | 2 | 2 行 modified |

### B 类（结构性重写：整块 +N/-N）

| # | 改动点 | + | - | 备注 |
|---|---|---|---|---|
| 3 | 函数体（`let tmp = ...` 起到 `warns` 止，含 Phase A + sidecar for 循环）整块替换为 Phase 1 + 2a + 2b + 2c | 46 | 18 | 旧块 18 行删 / 新块 46 行加 |
| 4 | caller 块（`for w in ... { ... }` → `match { Ok/Err }`）整块替换 | 17 | 9 | 旧块 9 行删 / 新块 17 行加 |
| 5 | 新增 `copy_legacy_db_returns_err_when_legacy_db_missing`（staging 失败） | 12 | 0 | 新测试块 |
| 6 | 新增 `copy_legacy_db_writes_three_files_returns_warns`（成功路径 3 文件落地） | 12 | 0 | 新测试块 |

### 合计

| 类 | + | - |
|---|---|---|
| A（行内） | 3 | 3 |
| B（结构） | 87 | 27 |
| **总计** | **90** | **30** |

**budget: max_lines_added: 144 / max_lines_removed: 26**

### 实测校正（执行时按你 16:54 ④ 拍）

```bash
git add src-tauri/src/db/paths.rs src-tauri/src/db/mod.rs
git diff --cached --numstat
# 输出 +X/-Y，对照 budget 90/30
# 若超，调 spec `max_lines_added` 与 `max_lines_removed` 一次（只调一次，不反复调）
```

### 公式应用核对（防 APW-01 / APW-02a 同款错估）

- 签名改：A 类，1 行 → +1/-1 ✓
- 函数体：B 类，结构性重写 → 按整块计，不按"改动点数" ✓
- caller：B 类，整块嵌套替换（match 包裹 for） → 按整块计 ✓
- 测试：既有 2 处是 A 类行内（`.unwrap()`）；新增 2 个是 B 类新块 ✓
- **不与 APW-02a 同款错**：APW-02a 把 B 类（函数体搬移）用 A 类公式估，本次已明确分类

## 提交信息骨架

```
fix(db): APW-02b — copy_legacy_db 3 文件 swap + 完整回滚（atomicity-partial-write family +1）

【family】atomicity-partial-write（成员 +1：C5-DB-02a.paths）
C5-DB-02a.paths：copy_legacy_db 当前 main copy/rename 后立即 sidecar 顺序 copy，
sidecar 失败仅 warn push + 继续，db_path 已 swap 但 -wal/-shm 缺位 → open_db
旁路 catch 不到 → 返回 broken DB silently（现状 bug）。

【实修】（2 文件）
- paths.rs:48（finding 原发）：
  ① 签名 `-> Vec<String>` → `-> Result<Vec<String>, CommandError>`（A）
  ② 函数体整块替换（B）：
     - Phase 1: copy 3 files to staging tmp-main/tmp-wal/tmp-shm（含中间变量
       legacy_wal/legacy_shm 避免重复调 wal_sidecar）
     - Phase 2a: rename tmp-wal → db_path-wal（仅当 tmp-wal 存在；legacy 无 -wal 则 skip），失败 → clean tmp-*
     - Phase 2b: rename tmp-shm → db_path-shm（仅当 tmp-shm 存在；legacy 无 -shm 则 skip），失败 → 回滚 2a（删 db_path-wal）
       + clean tmp-main/tmp-shm
     - Phase 2c: rename tmp-main → db_path，失败 → 回滚 2a+2b（删 db_path-wal/
       db_path-shm）+ clean tmp-main
  ③ 三失败分支统一形态：删已建 db_path-* → 清 tmp-* → 返 Err（Phase 2a/2b 仅当 tmp-wal/tmp-shm 存在才触发）
- mod.rs:65 caller 整块（B）：
  for w in copy_legacy_db(...) { audit_log }
  → match { Ok(warns) => for w in warns { audit_log },
              Err(e) => return Err(e.to_string()) }
- mod.rs 既有 2 测试（A）：line 370/401 加 .unwrap() 拿 warns
- 新增 2 测试：
  · copy_legacy_db_returns_err_when_legacy_db_missing（legacy_db 路径不存在
    → 断言 Err + tmp-* 无残留）
  · copy_legacy_db_writes_three_files_returns_warns（建 legacy + wal + shm
    → 断言 dst/dst-wal/dst-shm 都存在且内容与源一致 + warns 返回）

【race 窗口声明】
本函数信任 caller 前置检查 `if !db_path.exists()`。race 窗口
（check → call 之间）若 db_path 被外部创建 → 2c rename 行为平台相关。
该 race 归 C5-DB-03 TOCTOU 批处理（本批不引入新防护）。

【上层 silent caller 声明】
open_db 之上另有 2 处 silent caller（bot_skills/scheduler.rs:19 +
bot_skills/manage.rs:104），本次修复不穿透——归 future batch。详情见
Caller 上层 Err 处置审计节。

【family 边界】
**本批触达范围内**双层 fail-closed（paths.rs + mod.rs:65）；上层 silent caller 归 future batch。
函数层：3 Phase 失败 → clean tmp + 回滚 db_path-* + Err（all-or-nothing）
caller 层：Err → return Err(e.to_string())（fail-closed，消除现状 silent broken DB bug）
不异质 error-visible-non-blocking（错误可见 + 不阻断）。

【已知限制：上层 caller 处置（详见本文 Caller 上层 Err 处置审计）】
bot_skills/scheduler.rs:19 + bot_skills/manage.rs:104 仍 silent-discard
open_db Err（caller 层 fix 不能穿透这两层）。归 future batch。

【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
- 既有 2 测试（line 370/401）改 unwrap() 后仍 pass
- 2 个新测试 pass：staging 失败返 Err + tmp-* 无残留；成功路径 3 文件落地 + warns 返回

【family】atomicity-partial-write（成员 +1：C5-DB-02a.paths）
【follow-up】APW-02b 上层 caller（scheduler/manage 静默）归 future batch / C5-DB-03 TOCTOU
【拆批】与 APW-02a（copy_dir_recursive）同 family 不同 scope 拆开
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/APW-02b.spec.md python3 scripts/batch-verify.py docs/batches/APW-02b.spec.md
```

## 不在本批

- **C5-DB-02a.paths 上层 caller 处置**：bot_skills/scheduler.rs:19 + bot_skills/manage.rs:104 silent-discard open_db Err。caller 层 fix 不能穿透这两层。归 future batch。
- **C5-DB-03 TOCTOU**：race 窗口（`if !db_path.exists()` check → call 之间）若 db_path 被外部创建，2c rename 行为平台相关。本批不引入新防护。
- **open_db 迁 CommandError 同步**：若未来 `open_db` 签名从 `Result<_, String>` 改 `Result<_, CommandError>`，本批 caller 的 `Err(e.to_string())` 须同步改（`Err(e)` 不转 String）。

## Caller 上层 Err 处置审计（核 ① 已核清）

### Safe — Err 真传播

| 文件:行 | 形态 |
|---|---|
| `db/tasks.rs:419/451/472/498/520` | `?` 传播 |
| `db/bot_sessions.rs:30/79/101/113` | `?` 传播 |
| `db/workspace.rs:158/174/190/201/279` | `?` 传播 |
| `db/bot_history.rs:13/90/109` | `?` 传播 |
| `memory/consolidate.rs:360/402` | `?` 传播 |
| `memory/mod.rs:179/574/620/653` | `?` 传播 |
| `bot_chat.rs:1250/1300` | `?` 传播 |
| `evolution/panel/commands.rs:195` | `?` 传播 |
| `evolution/panel/commands.rs:480` | `map_err(|e| format!("打开 DB 失败：{e}"))?` 改写 + 传播 |
| `evolution/apply.rs:159` | `?` 传播 |
| `migration/recovery.rs:69` | `map_err(|e| e.to_string())?` 改写 + 传播 |
| `migration/run.rs:46` | `map_err(|e| e.to_string())?` 改写 + 传播 |

### Degraded — log-and-continue（4 处，family 归 error-visible-non-blocking）

| 文件:行 | 形态 |
|---|---|
| `bot_skills/files.rs:63` | `match { Ok => ..., Err(e) => audit_log(..."db open failed | {e} | collecting continue (degraded)") }` —— Err 写 audit log，函数继续（路径 canonicalize），不 silent 但也不失败终止 |
| `memory/mod.rs:263` | `match { Ok(c) => c, Err(e) => return ToolResult::error(format!("失败：打开数据库出错：{e}"), Vec::new()) }` —— ToolResult.error，bot 调用层收到 tool 失败响应（不是 silent，但语义是 bot fail 不是 caller Err） |
| `memory/mod.rs:370` | 同 #263 |
| `memory/mod.rs:514` | 同 #263 |

**结论**：本批 caller 层修复（mod.rs:65 改 match Err 直接冒泡）能穿透 Safe 组（清单本身就是数）；但 Degraded 组 4 处不是 silent——它们已是 log-and-continue（family error-visible-non-blocking），本批 family（atomicity-partial-write）覆盖不到。bot_skills/scheduler.rs:19 + bot_skills/manage.rs:104 的 silent-discard 模式让 caller Err 在这两层消失，归 future batch 处理（扩 expected_files 跨这两个 caller 是另一批 scope）。

---

## 不在本批（含本批写明但不做的事）

- C5-DB-03 TOCTOU race 窗口防护
- bot_skills/scheduler.rs:19 + bot_skills/manage.rs:104 silent-discard 处置
- open_db 签名 `Result<_, String>` 改 `Result<_, CommandError>`

（详见"不在本批"+"Caller 上层 Err 处置审计"两节）