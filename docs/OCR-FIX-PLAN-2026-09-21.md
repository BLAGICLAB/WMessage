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

### Phase 0 — 准备（一次性，先做）

- [ ] **验证基线**：跑 `scripts/test-all.sh`，把结果记进本文件。基线不绿就先记录已知失败，否则后续无法区分新旧问题。
- [ ] **triage 分类**：把 1145 条按 §3 规则预分类，产出跟踪表（见 §5）。
- [ ] **确认排除清单**：`src-tauri/vendor/**`（第三方，另立 Phase 5）；`src-tauri/dotnet/WmDocxRevisions/Program.cs`（生成物？需确认）。
- [ ] **定义 OCR 存量基线**：把本次 1145 条按 `path:line` 存成集合，复审时用于 diff 出"新增"。
- [ ] 提交本计划文件。

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
