# Batch Spec: EV-3b-B

## 目的

修 C5-EV-3b-B 5 条（jsonl 写 race / 非原子）：

1. **entry.rs:81**（high）：candidate append 无锁 + writeln! 多次 write
   可交错 → 候选池 jsonl 永久性损坏。
2. **record.rs:187**（high）：change append 同上。
3. **sandbox/io.rs:46**（high）：shadow/ab append 同上 + 无锁。
4. **emit.rs:105**（high）：dedup map MutexGuard 持锁跨 audit_event! 磁盘
   I/O 整循环——并发调用者全序列化在别人的 audit 写盘后。
5. **activation.rs:257→274**（high）：save_state read→mutate→std::fs::write
   无原子 rename 无锁——崩溃截断 bot-config.json（§12.7 要求保留其它字段），
   并发 save_state 最后写者胜静默丢转移。

## 人类可读摘要

- family: jsonl-write-atomicity（写竞态/非原子 → 锁 + 原子写）
- 覆盖 findings: 5
- 预估 diff: 7 files / +85/-67 lines（A 类，budget +110/-85）
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 6

## 核实的编译层事实（spec 前已核）

1. write_proposals 唯一调用点 = evolution/mod.rs post_consolidation（:56），
   **无锁**；其内部 read_all→dedup→append 是完整 RMW。panel 侧同文件读写
   走 EVOLUTION_STORE_LOCK——两套机制并存才是真 race 面。
2. EVOLUTION_STORE_LOCK 现为 panel/commands.rs 私有 static。方案：
   **上移到 evolution/mod.rs 为 pub(crate)**（同一把锁、同一约定 C3-4，
   注释同步搬），panel 改用 `super::`；post_consolidation 的
   write_proposals 调用包锁。entry/record append 内部**不**加锁
   （否则 panel 持锁调用死锁）。
3. save_state 调用点：due_notify.rs:266 + activation 内部；最终写改用
   `db::paths::atomic_write`（tmp+rename，仓既有模式）；RMW 加模块级
   static Mutex（仅 save_state 自洽）。**残余**：bot/config/io.rs 等其它
   bot-config 写者不在此锁内——跨写者统一锁 = 更大 scope，登记 follow-up。
4. emit.rs:104-135：锁内做 retain+contains+insert+audit_event!（磁盘写）。
   方案：锁内 retain + 分流（deduped 计数 / insert + 收集待写），**出锁后**
   再逐条 audit_event!。行为权衡：dedup 标记先于 audit 写（崩溃窗口对称
   互换：旧=写了 audit 没标记，新=标记了没写 audit），spec 声明。
5. append 原子化：serde_json 序列化 + 显式 `\n` 拼成单个 String →
   单次 `write_all`。POSIX O_APPEND 对常规文件的单次 write() 原子
   （offset 更新内核级）；**不引入 fs2/fd-lock 新依赖**——跨进程 flock
   残余风险注释声明（observ_run bin 与主进程同写场景）。
6. record.rs:193 fsync medium / 两个 error-enum medium **不在本簇 5 条内**：
   fsync 照 AP-01b 先例挂起（与仓既有 atomic_write 姿态一致）；error enum
   属错误类型化簇，均不动。
7. 既有 asserts：entry 14 / record 37 / sandbox io 11 / emit 19 /
   activation 42 / panel commands 19 / mod 0。

## 修法

- **evolution/mod.rs**：EVOLUTION_STORE_LOCK + lock_evolution_store 上移
  （pub(crate)，C3-4 注释搬全）；post_consolidation 的 write_proposals
  调用包锁（覆盖其内部 RMW）。
- **panel/commands.rs**：删本地锁定义，`use super::{lock_evolution_store}`，
  4 处持锁点不变。
- **entry.rs / record.rs append**：序列化 + `\n` 拼单 String → 单次
  write_all（+注释：调用方须持 evolution store 锁；O_APPEND 单写原子）。
- **sandbox/io.rs**：模块级 SANDBOX_IO_LOCK 包两个 append（shadow/ab 各
  自文件，一把锁足够，append 频率低）；同样单 write_all 化。
- **emit.rs**：锁内 retain+分流+insert，出锁后 audit_event! 循环。
- **activation.rs save_state**：模块级 SAVE_STATE_LOCK 包整个 RMW；
  最终写改 `crate::db::paths::atomic_write`。

## 红线

- 不加 fs2/fd-lock 新依赖；不加 fsync（照 AP-01b 挂起先例）
- 不动 record.rs error-enum / sandbox error-enum medium（非本簇）
- 锁上移不改 C3-4 约定本质（仍一把锁护 proposals+changes 两文件）
- read_all 全部保持无锁（外层锁已覆盖，内层加锁必死锁）

## spec 起草后自查三条

1. `expected_files` = 7：evolution/{mod.rs, emit.rs, activation.rs,
   candidate/entry.rs, change/record.rs, sandbox/io.rs, panel/commands.rs}。
   ripple = panel 锁 import 改路径（已列）。
2. budget = **A 类**：mod +20/-2、panel +3/-33、entry +7/-3、record +7/-3、
   sandbox io +22/-6、emit +18/-14、activation +12/-6：估 +89/-67，
   预留 → **max +110/-85**。
3. 5 条 findings 的 fix 字段均已写明 ripple（panel import）/无 ripple。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-3b-B",
  "family": "jsonl-write-atomicity",
  "expected_files": [
    "src-tauri/src/evolution/mod.rs",
    "src-tauri/src/evolution/emit.rs",
    "src-tauri/src/evolution/activation.rs",
    "src-tauri/src/evolution/candidate/entry.rs",
    "src-tauri/src/evolution/change/record.rs",
    "src-tauri/src/evolution/sandbox/io.rs",
    "src-tauri/src/evolution/panel/commands.rs"
  ],
  "max_lines_added": 110,
  "max_lines_removed": 85,
  "findings": [
    {"id": "C5-EV-3b-B-1", "file": "src-tauri/src/evolution/candidate/entry.rs", "line": 81, "fix": "append 改单次 write_all（显式 \\n）+ 调用方锁契约注释；锁本体在 evolution/mod.rs（store 锁上移 + write_proposals 包锁）；ripple: panel/commands.rs 锁 import 改 super::"},
    {"id": "C5-EV-3b-B-2", "file": "src-tauri/src/evolution/change/record.rs", "line": 187, "fix": "append 改单次 write_all（显式 \\n）+ 调用方锁契约注释；无其它 ripple"},
    {"id": "C5-EV-3b-B-3", "file": "src-tauri/src/evolution/sandbox/io.rs", "line": 46, "fix": "append_shadow/append_ab 单次 write_all + 模块级 SANDBOX_IO_LOCK；无 ripple"},
    {"id": "C5-EV-3b-B-4", "file": "src-tauri/src/evolution/emit.rs", "line": 105, "fix": "dedup 锁内仅 retain/分流/insert，audit_event! 移出锁后循环；dedup 标记先于 audit 写（崩溃窗口对称互换，spec 已声明）；无 ripple"},
    {"id": "C5-EV-3b-B-5", "file": "src-tauri/src/evolution/activation.rs", "line": 274, "fix": "save_state 加 SAVE_STATE_LOCK 包 RMW + 最终写改 db::paths::atomic_write（tmp+rename）；残余：bot/config 其它写者不在锁内（follow-up）；无 ripple"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/candidate/entry.rs": 14,
    "src-tauri/src/evolution/change/record.rs": 37,
    "src-tauri/src/evolution/sandbox/io.rs": 11,
    "src-tauri/src/evolution/emit.rs": 19,
    "src-tauri/src/evolution/activation.rs": 42,
    "src-tauri/src/evolution/panel/commands.rs": 19
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 6
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
