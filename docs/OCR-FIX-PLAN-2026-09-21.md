# OCR 全量审计修复计划（2026-09-21）

**来源**：OCR `full_scan` session `f301e717-506f-446d-a378-32c72335f58e`
（261 文件 / 1146 条 / 49m07s / 14.31M tokens / 模型 MiniMax-M3）

**原始产物**（已 gitignore，本地留存）：
- `docs/OCR-CODE-REVIEW-2026-09-21-fullscan.json`（1.5 MB，机器可读）
- `docs/OCR-CODE-REVIEW-2026-09-21-fullscan.html`（18 MB，离线 viewer）

---

## 0. 规模总览

| 区域 | 条数 | critical | high | medium | low |
| --- | ---: | ---: | ---: | ---: | ---: |
| `src-tauri/src`（Rust 核心） | 668 | 11 | 180 | 310 | 167 |
| `src`（前端） | 249 | 4 | 62 | 110 | 73 |
| `src-tauri/vendor`（第三方） | 93 | 7 | 31 | 34 | 21 |
| `src-tauri/` 其他（config/dotnet/tests） | 54 | 1 | 17 | 26 | 10 |
| `scripts` | 33 | 1 | 10 | 15 | 7 |
| 其他（根目录/配置/文档） | 48 | 0 | 11 | 21 | 16 |
| **合计** | **1145** | **24** | **311** | **516** | **294** |

> 另有 1 条结构不完整的记录（缺 `severity`），按低优先人工看一眼。
> 括号：`vendor` 里 7 个 critical / 31 个 high 属于 vendored 第三方代码，**默认不修**（见 Phase 5）。

**目标产出**：把 critical + high 里「真问题」清零；medium/low 按判定规则收敛（多数预期判 `wontfix`）。

---

## 1. 每批次的固定工作流（SOP，不可跳步）

用户要求的顺序，逐条落地：

| # | 步骤 | 具体动作 | 通过标准 |
| --- | --- | --- | --- |
| 1 | **判定** | 对每条的**根因**做 ponytail 判定（见 §3）：真问题 / 过度开发 / 误报 | 每条都有结论，记录到跟踪表 |
| 2 | **修复** | 只修真问题；根因修，不在调用点打补丁 | diff 尽量小；改完自查"能不能更少" |
| 3 | **验证** | 按 §4 验证矩阵跑对应链路 | 全绿；任何新增失败当批解决 |
| 4 | **OCR 复审** | `ocr review` 审**本次工作区改动**（commit 前） | 新增的 critical/high **必须当批处理**；存量发现不算新增 |
| 5 | **提交** | 一个批次一个 commit；消息含批次名 | pre-commit hook 通过 |

**OCR 复审命令**（commit 前，审 staged+unstaged）：

```bash
cd ~/Projects/wmessage
ocr review --format json --output /tmp/ocr-recheck-<批次名>.json
```

**批次熔断**：若复审产生的新发现 > 本批修复数的一半，说明改法有问题 → 停下来重新判定，不硬提交。

---

## 2. 阶段划分

### Phase 0 — 准备（已完成 2026-09-21）

- [x] **验证基线**：跑 `scripts/test-all.sh`（结果见下）。**基线不绿**，已知失败已登记。
- [x] **triage 分类**：产出 `docs/OCR-CODE-REVIEW-2026-09-21-baseline.json`（1052 条非 vendor，含 `path:line` + severity + category + 内容指纹）。
- [x] **确认排除清单**：`src-tauri/vendor/**` 已排除（93 条，Phase 5 单独决策）。`dotnet/WmDocxRevisions/Program.cs` **已判定为手写源码**（27940 字节，头部有 09-02/09-05 开发决策注释，git 历史 3 个功能 commit，publish 脚本只打包不生成）→ **不排除**，4 个 high 进 Phase 2。
- [x] **OCR 存量基线**：同上 baseline.json，后续复审用它 diff「新增 vs 存量」。
- [x] 计划文件已提交 `2f9041c`。

#### 基线结果（HEAD = `2f9041c`，工作区 clean）

| 检查 | 结果 |
| --- | --- |
| `cargo nextest`（1/3） | ✅ **1061 passed / 2 skipped** |
| `pytest tests-audit/`（2/3） | ❌ **10 failed / 28 passed / 1 skipped**（装好 pytest 后才第一次跑得到，见 Phase 0.5） |
| `tsc --noEmit` | ✅ exit 0 |
| `vitest run` | ❌ **2 files / 17 tests failed** |

**已知失败明细（17 条，全部为同一个环境回归）**

- `src/theme.test.ts` — 11 条、`src/components/WidgetApp.test.tsx` — 6 条。
- 共同根因：测试环境里 **`localStorage` 是 `undefined`**，两个文件的 `beforeEach` 都在 `localStorage.clear()` 上抛 `TypeError: Cannot read properties of undefined (reading 'clear')`，导致整文件用例全挂。
- 旁证：node 输出 `localStorage is not available because --localstorage-file was not provided` → 疑似 Node 26 自带的 `localStorage` 全局与 jsdom 的全局冲突（`vitest.config.ts` 已是 `environment: "jsdom"` + `setupFiles: ./src/test/setup.ts`，而 `setup.ts` **没有**兜底定义 `localStorage`）。
- 与本轮 OCR 修复无关：`src/` 前端代码本轮未改动。

### Phase 0.5 — 恢复验证基线（已完成 2026-09-21，commit `b0db1c8`）

- [x] **D1 装 pytest**：`python3 -m pip install --user pytest` → pytest 8.4.2（`/usr/bin/python3` 3.9.6 可见）。
- [x] **D2 修 17 个前端失败**：
  - 根因：Node 26 自带 `localStorage` 全局（未传 `--localstorage-file` 时为 `undefined`）遮蔽了 jsdom 的实现；两个测试文件的 `beforeEach` 都在 `localStorage.clear()` 抛 `TypeError`，整文件用例全挂。
  - 修法：`src/test/setup.ts` 按该文件既有风格（matchMedia/scrollTo/crypto 兜底）补内存版 `localStorage` → 11 个 theme + 5 个 WidgetApp 用例转绿。
  - 第 17 个（`bot on → off → on 切换`）暴露的是**测试与产品契约冲突**：测试断言 `bot_sessions_load` 只调 1 次，而 `ChatPanel.tsx:217` 的 `useEffect(..., [enabled])` 在 bot 重开时会重跑并再拉一次（该 effect 自带注释说明这是有意行为）。**决策 A —— 改测试承认「重开重载」**，同时用 `vi.waitFor` 取代固定 tick 数断言，用例名改为反映真实契约（ChatPanel 不重挂仍是要害）。
  - 验证：`vitest` 23 files / **229 passed**、`tsc --noEmit` clean。

#### 真实基线（HEAD = `b0db1c8`）

| 检查 | 结果 |
| --- | --- |
| `cargo nextest`（1/3） | ✅ **1061 passed / 2 skipped** |
| `pytest tests-audit/`（2/3） | ❌ **10 failed / 28 passed / 1 skipped** |
| `tsc --noEmit` | ✅ exit 0 |
| `vitest run` | ✅ **23 files / 229 tests passed**（Phase 0.5 修好） |

#### ⚠️ 新暴露的问题：pytest 10 个失败（D3，待决策）

装了 pytest 后才第一次跑得到这批一致性检查 → 暴露**长期存在的架构漂移**（重构搬走了代码，检查脚本没跟着更新，因为一直跑不了）：

| 脚本 | 失败项 |
| --- | --- |
| `audit_pre_step_pre_execute.py` | `test_pre_step_hit_triggers_start_skill`、`test_every_execute_tool_has_pre_execute_check`、`test_skill_on_step_called_in_execute_tool`、`test_audit_module_exists`（`classify_text` 不存在）、`test_post_execute_emits_structured_event`、`test_bypass_llm_switch_field_exists`、`test_bypass_llm_default_view_field` |
| `audit_module_map.py` | `test_tree_entries_point_to_existing_files`、`test_every_source_file_is_mentioned`、`test_declared_modules_have_doc_entry` |

**已核实与本轮 OCR 修复无关**：断言引用的是 `bot/` 分发、`audit.rs`、模块地图文档等我们没动过的地方（`tests-audit/` 里也没有 `status_label` 的引用）。

- [ ] **D3：这 10 个怎么处理？**
  - **A** 逐个更新检查脚本，使其匹配当前架构（工作量中等，但能让 pre-commit/push 门禁重新有效）
  - **B** 登记为「已知失败」，每批只要求「不新增失败」（Phase 1 可立刻开工）
  - **C** 先挑显然是「检查脚本过时」的几条批量修，其余登记

> Phase 1 是否开工取决于 D3：选 B/C 可立即开始；选 A 建议先做完再进 Phase 1。

### Phase 1 — critical（24 条，非 vendor 17 条）

按文件聚类，4 个批次：

| 批次 | 内容 | 条数 |
| --- | --- | ---: |
| **C1 文件系统/剪贴板** | `platform/copy_file.rs`(3) + `bot_fs.rs`(2，含 symlink 逃逸 + 阻塞 async runtime) | 5 |
| **C2 安全面** | `bot_skills/files.rs`(1，白名单 TOCTOU) + `capabilities/default.json`(1，`$HOME/**` 权限) | 2 |
| **C3 数据/进化** | `db/tasks.rs`(1，乐观并发 TOCTOU) + `evolution/observe/stop.rs`(1) + `evolution/panel/commands.rs`(1) + `eval/metrics.rs`(1) | 4 |
| **C4 前端+脚本+入口** | `WidgetApp/constants.ts`(2) + `App.tsx`(1) + `format.ts`(1) + `scripts/test-all.sh`(1) + `bin/observe_run.rs`(1) | 6 |

### Phase 2 — high（280 条非 vendor，163 文件）

按模块分批，每批 15–25 条，预计 **14–18 批**：

1. Rust：`api_handlers`、`db`、`bot*`、`evolution`、`py`、`memory`、`platform`、`migration`
2. 前端：`src/components`、`src/lib`、`src/*.ts(x)`
3. 脚本/配置

排序：`security` > `bug` > `performance` > `maintainability` > `test`/`documentation`。

### Phase 3 — medium（516 条，非 vendor 482 条）

**只修 `bug` + `security`**（预计 100–150 条）；其余（maintainability/style/perf 的主观项）走 ponytail 判定，多数判 `wontfix`。
预计 **8–12 批**。

### Phase 4 — low（294 条，非 vendor 273 条）

批量 triage，**只修零风险的**（typo、死代码、文档、明确的一行修法）；其余 `wontfix`。
预计 **3–5 批**。

### Phase 5 — vendor（93 条，独立决策）

`src-tauri/vendor/tiny_http` 是刻意 vendored 并打过补丁的（仓库有 `scripts/ci-guard-tiny-http-vendor.sh`）。
先看 `vendor/tiny_http/PATCHES.md` 能否**上游升级**替代自维护；否则按「接受现状 + 文档记录理由」批量关闭。**不逐条修。**

---

## 3. 判定规则（ponytail 落地）

对每条发现按顺序问，**停在第 1 个成立的档位**：

1. **这条需要存在吗？** — 防御性/投机性需求 → `wontfix`，一句话记理由。
2. **仓库里已经有做法了吗？** — 已有 helper / 既有惯例 → 复用它，不新建抽象。
3. **标准库 / 平台能力能做吗？** → 用它们。
4. **已装的依赖能解吗？** → 用它；**绝不为此新增依赖**。
5. **能一行修完吗？** → 一行。
6. 以上都不成立 → 最小可用改动。

**输出分类（跟踪表里的 `verdict` 列）**：
- `fix` — 真问题，按最小改法修
- `wontfix-design` — 设计如此，OCR 不了解约束
- `wontfix-yagni` — 修它属于过度开发
- `wontfix-vendor` — 第三方代码
- `defer` — 真问题但需产品/架构决策，单独提出来问

**特别检查**：OCR 是"全文件扫描"，容易把**既有设计**当缺陷（例如刻意保留的 `unimplemented!()`、vendored 补丁、测试 fixture）。判定阶段必须对根因做 `grep` 验证，不能只看评论文字。

---

## 4. 验证矩阵

| 改动范围 | 必跑 |
| --- | --- |
| 任意 | `scripts/test-fast.sh`（pre-commit 会自动跑） |
| `src-tauri/**` | `cargo fmt --check` + `cargo check` + `cargo nextest run`（`test-all.sh` 含） |
| `src/**` | `npx tsc --noEmit` + `npx vitest run` |
| 桥接 / 错误码 / 模块图 | `tests-audit/audit_tauri_bridge.py`、`audit_error_codes.py`、`audit_module_map.py` |
| **每批 commit 前** | `scripts/test-all.sh`（全量：cargo nextest + tests-audit + vitest） |

> 前端 `npm test` = `vitest run`；全量脚本是 `scripts/test-all.sh`。

---

## 5. 跟踪方式

- 本文件 = 单一事实来源。每批一行，`- [ ]` → `- [x]`，并追加实际 commit sha。
- 每批的 `verdict` 汇总（多少 fix / 多少 wontfix + 理由）写进批次小节。
- **红线**：**不要把批次号写进 `.rs/.ts/.tsx` 注释** —— pre-commit 的 `[0/N]` 审计批次号防线会直接拒提交，`audit-ok` 才能豁免。批次号只出现在本文件与 commit message 里。

跟踪表样例：

```
| 批次 | 范围 | fix | wontfix | 验证 | OCR复审 | commit |
| C1 | copy_file.rs + bot_fs.rs | 5 | 0 | ✅ test-all | ✅ 0 新增 | <sha> |
```

---

## 6. 红线与约束

- 不改公开 API / wire format（前端 `src/types.ts` 与 Rust enum 的字节级契约），除非 finding 明确要求且单独说明。
- 每批独立可回滚：一个批次一个 commit，不混入无关改动。
- 不修 `vendor/`（除 Phase 5 的升级决策）。
- 不新增依赖（ponytail 第 4 档）。
- 判定为 `defer` 的，攒到一批统一问，不打断节奏。
- 长程任务：状态全部落在本文件，跨会话可续。

---

## 7. 节奏建议

| 阶段 | 批次 | 说明 |
| --- | ---: | --- |
| Phase 0 | 1 | 半小时内 |
| Phase 1 | 4 | 最高价值，建议先完成 |
| Phase 2 | 14–18 | 主体工作 |
| Phase 3 | 8–12 | 选择性 |
| Phase 4 | 3–5 | 批量关闭 |
| Phase 5 | 1 | 单独决策 |

**建议每次会话推进 1–2 批**（含验证 + OCR 复审 + commit），完成后更新本文件再继续。
