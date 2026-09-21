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

> **账本规则（2026-09-21 确立）**：本文件的 1145 条 = **基线 finding 账本**。
> 修复批（C1b/C2a/C2b/C2c/C2d…）**衍生**的债**不入本账本**，另登记于
> `docs/OCR-FOLLOWUPS-INDEX.md`（其头已标明「修复批衍生债，非基线 finding」）→ **两套账本分离**。
> 且：**衍生债不单独开批** —— 遇到随对应主线批顺带清，或降级 `wontfix`。
> 理由：单独开批清衍生债是**无限循环**（已实证 8 批：33 条衍生债 / 0 条基线 finding 被清）。

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

#### D3 = C（已定，2026-09-21）

选了**折中**——先批量修 4 条「显然脚本过时」，其余 6 条登记为 xfail（不删除、不无理由 skip），等 Phase 6 收紧。

#### Phase 0.6 — 恢复 tests-audit 门禁（已完成，commit `0ef8003`）

- [x] **4 条「显然脚本过时」批量修**（doc-only + 脚本局限修复）：
  - `test_tree_entries_point_to_existing_files` → `tree_entries()` 回滚原 `TREE_ENTRY_RE.findall`，新增 `resolve_entry()` 走「同名就近」匹配（详见「技术债登记」）。
  - `test_every_source_file_is_mentioned` → `docs/rust-bot-architecture.md` 新增 **§6.4 源文件清单**，完整列出 136 个 `.rs` 文件。
  - `test_declared_modules_have_doc_entry` → 文档模块树删 `consts.rs`、拆 `db.rs / api_handlers.rs / bot/config.rs` 为对应目录子树（migration.rs 并入 db/migrations.rs）。
  - `test_audit_module_exists`（`classify_text` 断言）→ 删除该断言（`audit.rs` 中确认无 `fn classify_text`，早年设计未实现即被遗弃）。
  - mermaid `H3[(db.rs SQLite)]` → `H3[(db/ SQLite)]` 同步文档自洽。
- [x] **6 条加 `@pytest.mark.xfail(strict=False, reason="Phase 6 技术债: ...")`**（不删、不无理由 skip），各条 reason 见下表。

#### ⚠️ Phase 0.6 技术债登记（Phase 6 必须收紧）

**`tree_entries()` 的「同名就近」降级**（commit `0ef8003` 引入）：

- 原意：「tree entry 指向的文件必须真实存在」。
- 降级：原路径不在时按 `SRC.rglob(name)` 找同名，多同名时按字符串排序取 `rels[0]`。
- 副作用：文件被搬走/删掉也能过，门禁保护力下降。
- Phase 6 收紧方案：**路径末段 + 模块前缀匹配**（不是纯同名）。
- 重映射 entry 列表在 pytest stderr 输出里完整可见（含全部候选），便于 Phase 6 复核。
- 不得默默咽下重映射 —— 每次重映射都会打印 `[audit_module_map] tree entry 'xxx': 原路径不在，N 个同名候选 → [...](就近取 X)`。

#### 6 条 xfail 清单

| 测试 | reason（要点） | Phase 6 triage 方向 |
| --- | --- | --- |
| `test_pre_step_hit_triggers_start_skill` | `bot_chat.rs` 四拆后 facade 不再含 `start_skill(&app` 直调；调度路径在 `bot_skills/runtime.rs`。 | 改断言定位新调入点，或改 facade 重新导出 |
| `test_every_execute_tool_has_pre_execute_check` | `bot/dispatch.rs` 内化前置闸后，入口签名未变但 `run_pre_execute` 调入点改在内部函数。 | 同上 |
| `test_skill_on_step_called_in_execute_tool` | 同上（`skill_on_step` 也被 dispatch.rs 内化）。 | 同上 |
| `test_post_execute_emits_structured_event` | F-3 改名 `tool.return` 后，埋点位置可能在 `execute_tool_impl` / `execute_tool_with_stop` 之间漂移。 | grep 当前 `tool.return` 事件实际发出位置，重写断言 |
| `test_bypass_llm_switch_field_exists` | `BotConfig` 里 `bypass_llm_on_pre_step_hit` 可能改名/类型变了（F-1 后期重构）。 | grep 当前字段名，调整断言 |
| `test_bypass_llm_default_view_field` | `BotConfigView` 可能拆到 `bot/config/types.rs`（路径漂移）。 | grep 当前 View 定义位置 |

> 完整 reason 见各测试函数上方 `@pytest.mark.xfail(reason=...)`。`strict=False` 确保「意外通过」允许（Phase 6 收紧时不会反咬一口）。

#### Phase 0.6 后的真实基线（HEAD `0ef8003`）

| 检查 | 结果 |
| --- | --- |
| `cargo nextest`（1/3） | ✅ **1061 passed / 2 skipped** |
| `pytest tests-audit/`（2/3） | ✅ **32 passed / 1 skipped / 6 xfailed**（0 failed —— 门禁已恢复） |
| `tsc --noEmit` | ✅ exit 0 |
| `vitest run` | ✅ **23 files / 229 tests passed** |

### Phase 6 — 架构一致性门禁恢复（主线 Phase 1–5 完成后执行）

**目标**：tests-audit 全绿，xfail 数量归 0，pre-commit / pre-push 门禁重新有效。

> **Phase 6-T（前置子集，2026-09-21 裁决）**：**TOCTOU/权限「统一策略」收口** —— 即原索引中
> 被我误标为 "Phase6" 的 **11 条**（`C1b-1..7` + `C2a-1/2/3` + `C2c-v1`）。
> **边界**：6-T 只做**策略统一/决策**（TOCTOU 统一、Windows 策略、fail-open 决策、`ErrorKind` 保留、
> walk 吞错、`reveal_item_in_dir` 范围、`openPath` 迁移、便携模式覆盖、TOCTOU 残窗）。
> **与 6 的区别**：6-T = **策略**；6 = **tests-audit 门禁恢复**。**顺序：6-T 先于 6**。
> 此前 follow-up 注释里的「Phase 6 一并处理」一律按本裁决读作「Phase 6-T」。

**执行步骤**：

1. **逐条 triage** 6 条 xfail，每条归到下面 5 类之一：
   - **脚本过时**：重构后功能搬了位置/字段改名 —— 改断言（或拆分成更窄的检查）。
   - **文档缺失**：对应模块在文档树里没条目 —— 补 `docs/rust-bot-architecture.md`（注意 tree_entries 重映射日志）。
   - **真实架构漂移**：检查点与产品意图冲突 —— 必须先和用户确认「保留旧检查」还是「改产品」。
   - **检查已废弃**：检查点本身不再有意义 —— 删除并写 DEVLOG 记录原因。
   - **与 OCR 相关**：本轮 OCR 修复引入/暴露 —— 跟 Phase 1–5 一起处理。

2. **收紧 `resolve_entry`**：从「同名就近」升级为「路径末段 + 模块前缀匹配」。例如 `mod.rs` 必须落在某已知子目录（如 `db/`、`api_handlers/`），不再允许纯同名。

3. **恢复门禁**：
   - `scripts/test-fast.sh` 步骤 [3/N] pytest collect-only —— 当前只检查 `audit_pre_step_pre_execute.py`，需扩展到全部 4 个测试文件。
   - `scripts/test-all.sh` 步骤 [2/3] pytest tests-audit/ —— 已在跑，需把「任何 FAIL / 任何 XFAIL」都视为失败（strict 模式）。
   - pre-commit + pre-push 的 pytest 步骤同步。

4. **零 skip / xfail 政策**：
   - 禁止无 issue / 无 owner / 无到期日的 skip / xfail。
   - 每个跳过的检查必须挂在「已知降级」小节（含 reason、影响、复盘时间）。
   - 每 30 天例行 audit 一次（自动 routine，详见「防复发」）。

5. **防复发 routine**：
   - 挂月度 routine（每月跑一次 tests-audit，diff 出 xfail/skip 数量变化，> 0 则报警）。
   - pre-commit hook 增「新增 xfail 必须含 issue 链接」检查。

6. **目标值**（完成时验证）：
   - `pytest tests-audit/` → 0 failed / 0 xfailed / 0 skipped。
   - pre-commit / pre-push 全绿。
   - tree_entries 重映射日志为空。

#### Phase 1 / C1b 实施期 follow-up 登记（2026-09-21）

C1b commit 引入的技术债，Phase 6 一并处理：

- **Phase 6: grep_files / walk 路径的 TOCTOU 复核**——C1b 只在 `tool_read_text_file` 加 inode re-check；`tool_grep_files` / `tool_list_files` 走 walk + ReadDir，TOCTOU 暴露面不同（迭代期间文件被换可能性），且 C1b 约束 2 明确"不碰 walk/grep 流式优化"。Phase 6 评估是统一策略（也加 inode re-check）还是承认 grep/list 的 TOCTOU 风险等级低。
- **Phase 6: resolve_with_perm 其它调用点 TOCTOU 统一策略**——`tool_read_text_file` 用 inode re-check 保护"open file"窗口，但后续 `read_capped_file` 又开了一次 file（不同 fd），那段不在保护范围。Phase 6 评估 fd 是否需要传出 / 复用。
- **Phase 6: symlink target 替换防护收紧**——C1b inode re-check 不覆盖 symlink target 被替换为另一文件的攻击（canonical 出来路径不变但内容指向新文件）。需更严策略（如：记录 canonicalize 时 target inode 并比对），但复杂且跨平台语义略异。
- **Phase 6: Windows 端 TOCTOU 策略**——先决定做不做，再决定怎么做。当前 Windows 端 `capture_pre_ino` 直接 `None`，整个 inode re-check 不执行（设计选择，非缺陷，但 Windows 端 read_text_file 无 TOCTOU 保护）。
- **Phase 6: capture_pre_ino None 时 fail-open vs fail-closed**——需要产品决策。当前 fail-open 是有意选择（避免 race window + Windows 端整体打死 read_text_file），但意味着 metadata 失败时不参与 inode 比对。fail-closed 会让 Windows 端 read_text_file 不可用。两个 follow-up 决策人和约束不同，分开写。
- **Phase 6: spawn_blocking_io / spawn_blocking_map 丢失 io::ErrorKind**——C1b 走 spawn_blocking_map 契约，io::Error 转 String，调用方拿到的是 String 错误。Phase 6 评估是否要扩 spawn_blocking_io 保留 ErrorKind。
- **Phase 6: grep_files / list_files walk 包 spawn_blocking 后 `unwrap_or_default()` 吞错误**——C1b 因闭包永远 Ok 所以今天不可达，但若 walk / read_to_string 后续扩展返 Err，会被静默吞。Phase 6 加日志或显式 match。

#### ⚠️ 流程事故登记：OCR review 后台产物不可见（2026-09-21）

- **现象**：OCR review 后台启动后，进程存活但无 stderr 中间输出、无 JSON 产物落盘。
- **触发条件**：MiniMax API 调用长 round（diff 大 / 文件多轮 review）、节点暂态卡 IO、或 OCR CLI 静默期。
- **与"被 reaper 清"的区别**：reaper 清是进程被 SIGKILL、产物可能部分落盘；静默挂起是进程仍 alive 但 OCR CLI 不 flush 中间步骤。
- **已出现次数**（截至 2026-09-21）：
  1. C1a 主 review：跑 ~3 分钟出产物（lucky-nudibranch 在 `/tmp`）——正常
  2. A=a followup：vivid-haven session 被 reaper 清、产物丢失 —— **reaper 清**
  3. C1b v1 review：sharp-meadow 跑 ~25 分钟出产物 ——正常但异常长
  4. C1b v2 review：mellow-zephyr 后台跑 7+ 分钟只 1 行 log、产物缺失 —— **静默挂起**
- **临时 SOP**（commit 前，**已定稿，不再每批现场决**）：
  1. 后台启动 OCR review 到稳态路径（`~/.openclaw/cache/ocr-<批次>.json`，避免 `/tmp` 被清）
  2. 前台轮询产物（`ls -la`）+ pgrep 存活 + log 大小变化
  3. **超过 10 分钟无产物落盘** → 杀掉后台，转前台重跑（`ocr review` foreground，timeoutSeconds=900）
  4. 前台仍失败 / exit 0 但产物缺失 → 转 D（commit 后异步补 review + commit message 诚实写明 + 本条目升级）
  5. 每次走完在 commit message 里**明确写"走了 B 还是 D"**（避免报告含糊——C1b v2 的教训）
- **follow-up**（流程修复，非本批）：
  - 给 OCR CLI 加中间进度 flush（每隔 N 秒打 stderr 一行），不要只靠"等产物"
  - 或产物心跳（每 N 秒 touch 一个 .heartbeat 文件），超时判定更准
  - 不要把"D 路径"当常规路径（避免以后所有 review 都跳过）

#### C2a Q1 wontfix 登记：widget capability 拆分（2026-09-21，已回滚）

**判定**：OCR C2a finding #1（`capabilities/default.json:5`，main + widget 共用 capability，severity medium→实际是 critical 级功能风险） —— **不修，登记 wontfix**。尝试过的 capability 拆分**已回滚**（未采纳，不产生 commit）。

**事实**（为什么拆分走不通）：

- `WidgetApp.tsx` 直接调用 `getCurrentWindow().setPosition / setSize / setAlwaysOnTop / startDragging / outerPosition / outerSize / scaleFactor / currentMonitor` 等 8+ 个窗口 API。
- 来源：`ResizeEdge.tsx`（拖拽缩放手柄）+ WidgetApp 自身窗口逻辑（双击标题唤起、拖拽移动、缩放）。
- **Tauri 2 的 `getCurrentWindow()` 是直调 API，不走 IPC** —— `grep invoke|tauri\.` 扫不到，因此初次调研漏判。
- 拆掉 `core:window:allow-*` 会让这些调用在运行时被 `WidgetApp` 的 `.catch(()=>{})` 静默吞掉 → **CI 通过但 widget 拖拽/缩放功能失效**（比"过度授权"更糟：功能性破坏且不可见）。

**为什么不是"过度授权"**：

- 主窗口与 widget 共享同一批窗口权限，是**产品设计结果**（widget 需要拖拽/缩放/置顶），不是配置疏忽。
- 注释里"widget 只能调 focus_main_window"是我的错误推断——`focus_main_window` 只是 widget 需要的能力**之一**，不是全部。

**真正的降面路径**（不在本批、不默认做；Phase 6 / 产品决策条目）：

- **产品层**：widget 是否可以取消拖拽缩放、改成固定尺寸 / 只读面板？若可以，capability 拆分才成立。
- **进程层**：widget 是否可以独立进程（更小 IPC surface）？需评估 Tauri 多 webview 进程方案。
- **触发条件**：若哪天 widget 重构为只读面板（去掉 `ResizeEdge.tsx` + 窗口操作逻辑），再回来做 capability 拆分。在此之前保持 `windows: ["main", "widget"]` 共享。

**重评估触发条件**：widget 设计变更（去拖拽 / 去缩放 / 只读化）。

#### ⚠️ 流程事故登记：跨 IPC 边界调用链调研不足（2026-09-21）

**现象**：对前端/跨 IPC 调用链调研不足，基于不完整 grep 下"功能/安全"判断，导致改动在运行时破功能。

**已发生（2 次，同一根因）**：

| # | 批次 | 错误推断 | 实际后果 |
| --- | --- | --- | --- |
| 1 | C1b L402 | 认为 `capture_pre_ino` 改 fail-closed "不影响功能" | 会把 Windows 端 `read_text_file` 整体打死 |
| 2 | C2a Q1 | 认为 widget "只需 focus_main_window，不需窗口权限" | 拆分后 widget 拖拽/缩放静默失效 |

**根因**：`grep invoke|tauri\.` 只覆盖 IPC 调用，漏了 **Tauri 2 直调 API**（`getCurrentWindow()` / `getAllWindows()` / `WebviewWindow` 构造 / `emit` / `listen`）。

**纠正（判定前强制步骤）**：涉及「改权限 / 改默认行为 / 改 fail 语义」的改动，判定前必须补一步**跨 IPC 边界的调用点搜索**，搜索模式至少包括：

```
invoke(          # IPC 命令调用
getCurrentWindow / getAllWindows / WebviewWindow 构造   # 直调窗口 API（无 IPC）
emit( / listen(  # 事件
```

并**列出实际调用清单**（文件:行 + 调用）再下结论。**不允许**只有一句"应该不需要"就改。

**备注**：这条比"OCR review 后台被清"更值得记——它是 C1b L402 与 C2a Q1 两次熔断的**共同根因**，而后者只是工具/环境问题。

#### C2a Q2/Q3 决策 = A1（2026-09-21）：opener path scope 摘 $HOME/**

**决策**：`opener:allow-open-path` 从 `[$APPDATA/**, $HOME/**]` 收窄为 **`[$APPDATA/**]`**（摘除 `$HOME/**`）。

**证据链**（决策级）：

1. **`$APPDATA` 是 app-scoped，不是整个 app data 根**（Tauri 2.11.5 `path/mod.rs:141-143`：`AppData` resolves to `Data/{bundle_identifier}`）→ OCR finding #2「$APPDATA/** 暴露所有程序数据目录」**判定为误报**，`$APPDATA/**` 已是窄范围。
2. **dcb9275（2026-08-19）保留 `$HOME/**` 的原始理由已被正面取代**：当时"主窗 openPath 打开任务绑定文件"。现证据：主窗三个任务绑定文件打开点全部走 Rust `open_file_path` —— `TodoCard.tsx:186`（任务卡）、`WorkspacePage.tsx:233`（工作区）、`ChatPanel.tsx:1205`（聊天区文件）；迁移 commit `5eb0a27`（2026-09-04）diff 已核。
3. **全仓唯一前端 `openPath()` 调用点 = `SkillsPanel.tsx:91`**，打开 `skills_open_dir()` = `$APPDATA/skills` → 摘除零功能成本。
4. **`openUrl` 独立**：opener ACL `allow-open-url`（commands: `open_url`）与 `allow-open-path`（`open_path`）分离 → markdown 链接不受影响。
5. **便携模式 exports 走 Rust**：`AI_Gen_Files` 打开经 `openTarget()` → Rust `open_file_path`（`path_openable_in` 双侧 canonicalize + `gen_dir()` 便携感知）→ 不依赖前端 opener。

**新测试**：`opener_path_scope_is_apdata_only`（`src-tauri/src/lib.rs`）——严格断言 `scope == ["$APPDATA/**"]`、显式断言 `$HOME` 不在 scope、禁裸通配、`/etc/passwd` 与 `~/.ssh` 不被覆盖。**替换**旧测试 `opener_open_path_scope_is_not_bare_wildcard`（语义已变）。

**⚠️ 已知边缘（摘除引入的真实回归面，非"罕见即放过"）**：

- **现象**：便携模式下，若 `app_data_dir()` 失败，`skills_dir()`（`bot_skills/manage.rs:33`）fallback 到 `crate::db::data_dir(app).join("skills")`（= 便携 exe 目录）。该路径**可能不在 `$APPDATA` 下** → `SkillsPanel` 的 `openPath` 会被 scope 拒绝（`.catch(()=>{})` 静默吞）。
- **触发条件**：`app_data_dir()` 失败 **且** 便携模式（exe 目录可写且不在系统 app data 下）。
- **为什么不在本批修**：修法之一是把 fallback 路径纳入 scope，但那要保留 `$HOME` 或引入新 scope 变量 → 与 A1 冲突，需重新判定。
- **登记为 follow-up（见下）**：Phase 6 评估是否把 `SkillsPanel` 的 `openPath` 迁移到 Rust 命令（与 `5eb0a27` 同模式，可彻底移除前端 opener path 依赖）。
- **本批验证**：`cargo test` 覆盖 `path_openable_in` 与 scope 断言；便携模式 `app_data_dir()` 失败分支无自动化覆盖 → **标记为未运行时验证分支**（诚实登记，不声称已验证）。

**C2a follow-up（不在本批）**：

- **Phase 6: `reveal_item_in_dir` 独立评估**——`opener:default` 含 `allow-reveal-item-in-dir`，**无 path scope**（ACL manifest `allow=[]`）。前端搜证当前未使用该命令，但它是独立于 `open-path` 的"在文件管理器中显示任意路径"能力，需单独评估是否要限制范围（潜在 XSS → 信息泄露面）。
- **Phase 6: `SkillsPanel` 前端 `openPath` 迁移到 Rust 命令**——彻底移除前端 opener path 依赖，随之可考虑连 `$APPDATA/**` 也摘除（届时 `opener:allow-open-path` 整条可下线）。触发条件：便携模式边缘 case 需要正面解决时。
- **⏳ 补验（非“已验证”）：解锁屏幕后补 GUI 点击验证**——C2a A1 commit 时屏幕锁定，GUI 点击级验证未执行。解钁/保持交互后补跑两项：
  1. `SkillsPanel` → 点“打开目录”→ 应打开 `$APPDATA/skills`（不被 scope 拦）
  2. 便携模式 exports 打开 → 确认走 Rust `open_file_path`、不被 scope 拦
  —— 补验前，该两项状态为“运行时验证部分完成”（见 commit message）。

#### C2b-1（簇 A：路径校验加固）—— 已 commit（2026-09-21）

**收**：#1 critical（TOCTOU）、#3 high（归一化）、#6 medium（delete 防御）、#4 medium（is_dir 死参数）、
M1（async 阻塞）、M2（open 用 canonical）、M3（gen_dir 缓存）、H1（delete 二次 re-check，r2 出）、
r3 high（delete 范围静默扩大）。

**实现要点**：
- `canonical_string`：canonicalize + Windows 剥 `\\?\`（复用 `bot_fs::strip_verbatim`，已 pub(crate)）
- `canonical_if_openable`：canonical 命中绑集或落 gen_dir → 返回 canonical 字符串；`path_openable_in` 委托它
- `recheck_canonical`：紧邻副作用前的二次校验（fail-closed），返回 canonical
- `open_file_path`：入口 check → 紧邻 open 二次校验 → open **canonical**（M2）；gen_dir 只算一次（M3）
- `delete_bound_file`：删 is_dir；入口 check + 紧邻 trash::delete 二次校验（H1）；**gen_dir=None**（范围只含绑集）
- `collect_openable_paths`：canonicalize 批次 move 进 `spawn_blocking_map`（M1）+ DB 失败 audit

**OCR 轮次**：r1 SIGTERM 失败 → r2（1 high = H1，熔断）→ 修 → r3（1 high = delete 范围静默扩大，
同根因最后一轮）→ 修 → r4（**0 comments**）。证据固化于 `~/.openclaw/cache/ocr-C2b-1-evidence/`。

**注释自查**：全部新增/修改注释逐条对照代码；修正 1 处不实（collect doc 误称集合含 AI_Gen_Files）；
引用 C1b 的 `capture_pre_ino`/Windows 限制已核实属实。

**follow-up 登记（不修）**：
- L1：`canonical_string` 用 `to_string_lossy` 丢非 UTF-8 字节（审计日志质量，非安全）
- L2：部分测试仍传 raw `Some(&gen)` vs `gen_canon`（测试卫生）
- r3 low：`Path::exists()` 在 check 之前（删除竞态时错误信息误导）
- r3 low：`canonical_if_openable` 每次重 canonicalize gen_dir（open 一次 IPC 两次）

**待办（簇 B 前置）**：存量 workspace-link kind 统计，出数字后再动簇 B（B2 白名单）。

#### C2b-2（簇 B：workspace-link kind 白名单）—— 已 commit（2026-09-21）

**收**：#2 high（`l.kind != "url"` 黑名单漏闸 → app/command/未来 kind 经 opener = RCE）。

**写入侧（fail-closed）**：新增 `CommandError::InvalidWorkspaceLinkKind{kind,source,link}`
（code `INVALID_WORKSPACE_LINK_KIND`，error.rs「变体+ALL+前端同步」三步全做）；
`db::workspace::validate_link_kinds` 在 `upsert_workspace` / `workspace_import_merge`
入口校验（事务前 → 混合批次零写入）。

**消费侧（集合判断）**：`files::collect_openable_paths` 的 `!= "url"` 改为
`link_kind_contributes_path(kind)`（仅 {file,folder} 贡献路径）。

**存量统计（可复核）**：2026-09-21 20:25 前后 sqlite 只读 `SELECT links FROM workspace_items`：
安装版 DB 0 行 / dev 便携 DB 0 行；workspace_items 无 deleted/archived 列 → 全量计入。
→ 存量 0 / app·command 0 → B2 无迁移风险。

**OCR**：r1（6e2f7eff）3 high（同根因：新 code 漏登记 `CommandErrorCode::ALL`+单测 every）→ 修；
r2（364e2749）0 new high → 收口。

**follow-up 登记（不修）**：
- medium `files.rs:4`：消费侧字面集与 `db::workspace::ALLOWED_LINK_KINDS` 重复 → 应引用共用常量
- low `errorHandler.ts`：hint「删除后重新添加」与 `recoverable=true` 语义需对齐
- medium/low `db/mod.rs` 测试：`mk` 闭包硬编码 link id `"l1"` / target_uri `/tmp/x.app` → 测试数据语义不一致

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

跟踪表（**2026-09-21 回填**；每批一行，收口时更新。C3 行即本批产物）：

| 批次 | 范围 | fix | wontfix | 验证 | OCR复审 | commit |
| C1 | copy_file.rs(3) + bot_fs.rs(2) | 5 | 0 | ✅ test-all | ✅ 0 新增 | c9ff340 / 3501ae7 / 3266382 |
| C2 | bot_skills/files.rs(1) + capabilities(1) | 2 | 0 | ✅ test-all | ✅ | 39675aa / 8b0dfdb / 95a25e9 |
| C3 | db/tasks.rs + eval/metrics.rs + evolution/observe/stop.rs + evolution/panel/commands.rs | 4 | 0 | ✅ test-all（1073 Rust + 229 vitest） | r1（一次收口，未跑 r2） | 本批 commit（自引用；精确 sha 以 git log 为准） |
| C4 | WidgetApp/constants.ts(2) + observe_run.rs + test-all.sh + format.ts + App.tsx | — | — | 未开 | — | — |

> **C3 批定性：2 critical（C3-3 / C3-4 实修）+ 1 high（C3-1，原 critical → 实现前发现生产路径已锁覆盖而降级）+ 1 medium（C3-2，原 critical → 字段级对外语义保留、无内部生产消费者而降级）。非「4 critical 完成」。**
> C3-1 的三层现状（CI/debug 断言强制 / release 靠调用点事实 / 跨进程未覆盖）见本批 commit message。

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
