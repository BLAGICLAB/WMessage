# 大文件拆分 · 第二轮（2026-10-25 续）

来源：第一轮（`docs/FILE-SPLIT-PLAN-2026-10-25.md`）把 18 条超限债务全部清账，
白名单清空。但**债务是平移的，不是消失的**——第一轮结束时全仓 286 个门禁口径内
文件的行数分布是：

    p50=394  p75=681  p90=1058  p95=1187  p99=1452  max=1487

阈值 1500 卡在 p99 之上，只放行百分之一的最长尾巴，且**余量已经很薄**：
离上限最近的 `bot/services/tests.rs` 只剩 13 行。本轮处理的就是这一圈
「贴着线」的 10 个文件——它们现在不改也会在下次功能开发时被门禁拦下。

本轮是**第一轮方法的复用**，不是新方法：仍是纯搬移，仍是每文件一个 commit，
仍走 §四 那张固定收尾清单。第一轮踩出来的 8 个坑见 `FILE-SPLIT-PLAN-2026-10-25.md`
的 §十 / §十一，**先读那两节再动手**。

## 〇、一句话结论

10 个文件分三类，**难度差一个量级**：

| 类别 | 条数 | 手法 | 风险 |
|---|---|---|---|
| **A 类·测试外移** | 6 | 内联 `#[cfg(test)] mod` 整块搬成同层子文件 | 极低（第一轮已做过 8 遍） |
| **B 类·纯测试文件按节切** | 2 | 已是测试专用文件，按主题分文件 | 极低 |
| **C 类·生产切分** | 2 | 按职责切子模块 + 门面逐项 re-export | 中 |

**A + B 类 8 条先做**（当天可完成，零行为变化），**C 类 2 条最后做**。

## 一、逐文件切法

| # | 文件 | 现 | 结构 | 手法 | 目标 |
|---|---|---|---|---|---|
| 1 | `src/bot/services.rs` | 1332 | 门面却持有 534 行启动 + 274 行停止 + 152 行序列化 | **C**：切 `services/{start,stop,serialize}.rs`，门面只留类型/常量/转出 | 门面 ~310 |
| 2 | `src/bot/services/tests.rs` | 1487 | 纯测试文件，8 节 | **B**：按节切 `services/tests/{logs,state,gate,canary,adjudicate}.rs` | 5 文件 |
| 3 | `src/memory/extract.rs` | 1452 | 850 生产 + 602 内联测试 | **A**：测试外移 → `memory/extract/tests.rs` | 850 |
| 4 | `src/bot_skills/parse.rs` | 1401 | 740 + 661 | **A**：→ `bot_skills/parse/tests.rs` | 740 |
| 5 | `src/bot_skills/scheduler.rs` | 1401 | 609 + 792 | **A**：→ `bot_skills/scheduler/tests.rs` | 609 |
| 6 | `src/bot_skills/runtime.rs` | 1365 | 822 + 543 | **A**：→ `bot_skills/runtime/tests.rs` | 822 |
| 7 | `src/bot_archive.rs` | 1404 | 987 + 417 | **A**：→ `bot_archive/tests.rs` | 987 |
| 8 | `src/bot_py.rs` | 1379 | 门面（转出 `crate::py::*`）+ 1227 行内联测试 + 1 个内联测试块 | **A**：→ `bot_py/{tests,exiting_tests}.rs` | ~35 |
| 9 | `src/bot_web.rs` | 1400 | 1398 生产（测试已在 `bot_web/tests.rs`） | **C**：切 `bot_web/{search,html,fetch}.rs` | 门面 ~200 |
| 10 | `src/db/tests.rs` | 1461 | 纯测试文件，4 节 | **B**：切 `db/tests/{pragmas,legacy_wal,migrate_json,copy_dir}.rs` | 4 文件 |

合计 14,082 行 → 拆成 20 个文件，全部 ≤ 1500。

**执行顺序（有依赖，不能乱）**：

```
1 → 2      #1 先定门面形状，#2 的测试要经门面取用（门面变了，测试的可见面就变了）
3 → 4 → 5 → 6 → 7 → 8    # A 类，彼此不相交
9 → 10                   # C 类最后
```

**#1 必须先于 #2**：`bot/services/tests.rs` 经 `use super::*` 取用门面的命名空间。
先拆门面再拆测试，测试要补的 re-export 一次性加齐；反过来做要改两遍。

## 二、C 类 2 条的切点（比第一轮难，先看清）

### #1 `src/bot/services.rs`（1332）

它已经是门面（`gate` / `logs` / `observe` 三个子模块 + 逐项转出），但**启动 /
停止 / 序列化三块生产逻辑还留在门面里**——门面应该只有类型、常量、use 与转出。

```
src/bot/services.rs (~310)      类型（ServiceState/ManagedService/ServiceEntry）
                                + 常量 + 闸/开关 + 逐项转出
src/bot/services/start.rs (~540) do_start + 就绪等待 + 失败归因 + 启动参数
src/bot/services/stop.rs (~290) 停止信号阶梯 + 退出清理 + 登记表访问
src/bot/services/serialize.rs (~170) service_to_json + services_snapshot_json
```

**难点**：`bot/services/tests.rs` 用到 `do_start` 之外的一批内部项
（`enforce_listening_gate` / `sample_resources` / `evaluate_resources` /
`make_service_dir` / `open_append` / `read_tail_bytes` / `tail_lines` /
`truncate_stream` / `reserved_ports` / `violation_message` /
`spawn_startup_listening_watch` / `LAST_SAMPLE_OK`）。门面得把它们**继续留在
自己的命名空间里**（私有 `use` 或 `#[cfg(test)] use`），否则测试看不见。

### #9 `src/bot_web.rs`（1398 生产）

```
src/bot_web.rs (~200)         类型（TimeRange/SearchOpts/UrlGateError）+ 门面转出
src/bot_web/search.rs (~360)  web_search / web_search_with_config / tavily / brave /
                              bing / baidu 后端与路由判定
src/bot_web/html.rs (~380)    HTML 标签解析内核 + parse_bing / parse_baidu
src/bot_web/fetch.rs (~460)   check_public_url / fetch_text / jina reader / IP 私有判定
```

**难点**：`bot_web/tests.rs`（647 行）经 `use super::*` 取用**大量私有辅助**
（`strip_tags` / `decode_entities` / `extract_main_content` / `parse_brave_results` /
`is_private_host` / `parse_alt_ipv4` …）。这些都要经门面**转出或保留可见性**，
否则测试编译不过——这是本轮最容易漏的一处（门禁会在编译期兜住，但要先想到）。

## 三、每批的固定收尾清单

沿用第一轮 `FILE-SPLIT-PLAN-2026-10-25.md` §四，**加一条**：

```
□ python3 scripts/verify_pure_move.py <原文件> --manifest <清单> --base-dir <目录>
  （纯搬移核对：清单声明的每一段原文必须在目标文件里逐字找得到；
   白名单化前缀用 --strip-prefix 声明，获准改动段用 --allow-chunk 声明）
```

清单（chunk manifest）每行 `<相对路径>:<起>-<止>`，行号指**原文件** 1-based 闭区间。
拆分脚本直接产出清单，核对器再验一遍——**「纯搬移」从此是可证明的，不靠人眼过 diff**。

其余固定项：cargo fmt --check / clippy `--all-targets -D warnings` /
cargo nextest 全量 / npm test / tests-audit 六件套 / audit_no_eval.py /
更新 `audit_file_size.py` 的 WHITELIST（若有条目）/ 更新 `docs/rust-bot-architecture.md` 三处
（模块树条目 + §6.4 清单 + 开头散文数字）/ 人眼过一遍 diff。

## 四、这一轮特有的坑（第一轮已列的 8 个坑仍然适用）

1. **`include_str!` 路径随文件深度变化**——测试文件下沉一层，
   `../../tests/fixtures/x.json` 要改成 `../../../tests/...`。
2. **模块树同名歧义**——新增 `bot_web/search.rs` 之类若与既有条目同名，
   `audit_module_map.py` 会拒绝「同名就近」匹配，要求文档树写全路径。
3. **门面可见性翻译**——`pub(super)` 在子模块里指向**该子模块的父级**，
   不是原来的模块；搬运时按「原可见域 = 原模块」逐项翻译。
4. **纯测试文件拆成目录形态**——父文件 `src/db/tests.rs` 变成只放
   `#[cfg(test)] mod X;` 的门面，子文件在 `src/db/tests/X.rs`。
   `bot/test_files.rs::test_only_files` 靠**模块声明**识别测试专用文件，
   所以门禁能自动认出来（不靠文件名猜）。

## 五、进度回填

| # | 文件 | 状态 | commit | 备注 |
|---|---|---|---|---|
| 1 | `bot/services.rs` | ✅ 已完成 | 待填 | C 类，门面先行。1332 → 门面 468 + start 522 / stop 257 / serialize 114。**类型留门面**（StartRequest / StartOutcome / StopOutcome）、**登记表访问留门面**（lock_table / list_all 是三个子模块共同依赖的 hub），其余实现按面切走 |
| 2 | `bot/services/tests.rs` | ✅ 已完成 | 待填 | B 类。1487 → 门面 62 + 七个主题子文件（adjudicate / canary / integration / logs / observe / port_gate / state）。**门面只留共用辅助**（`long_running_launcher` / `which`，被三个节共用）与 `#[cfg(test)] mod X;` 声明——`#[cfg(test)]` 不能省：`bot/test_files.rs::test_only_files` 靠模块声明识别测试专用文件，写成裸 `mod X;` 子文件会被当生产代码扫 |
| 3 | `memory/extract.rs` | ✅ 已完成 | 待填 | A 类。1452 → 850 生产 + `memory/extract/tests.rs` 601。**去缩排要「智慧」**：Rust 1.83 起普通字符串字面量允许裸换行，内容**包含**后续行的前导空白——去缩排会把字符串内容改了，所以多行字符串 / raw string / 块注释的内部行一律不缩 |
| 4 | `bot_skills/parse.rs` | ✅ 已完成 | 待填 | A 类。1401 → 742 生产 + `bot_skills/parse/tests.rs` 660 |
| 5 | `bot_skills/scheduler.rs` | ✅ 已完成 | 待填 | A 类。1401 → 610 生产 + `bot_skills/scheduler/tests.rs` 789 |
| 6 | `bot_skills/runtime.rs` | ✅ 已完成 | 待填 | A 类。1365 → 823 生产 + `bot_skills/runtime/tests.rs` 540 |
| 7 | `bot_archive.rs` | ✅ 已完成 | 待填 | A 类。1404 → 989 生产 + `bot_archive/tests.rs` 416 |
| 8 | `bot_py.rs` | ✅ 已完成 | 待填 | A 类，门面只留转出。1379 → 41 门面 + 三个测试子文件（tests 1226 / exiting_tests 43 / platform_tests 70）。模块路径不变（`bot_py::tests` 等），`super::X` 仍经 pub use 解析为 `py::X` |
| 9 | `bot_web.rs` | ⬜ 未开始 | | C 类，测试可见面最复杂 |
| 10 | `db/tests.rs` | ⬜ 未开始 | | B 类 |
