# UI 改造 U1–U4 交接文档（含开工提示词）

> 2026-09-30 · 供新会话接手执行 UI 改造战役。方案依据 `docs/UI-REDESIGN-PROPOSAL-2026-09-30.md`（已拍板）。
> 流程与审计修复战役（AUDIT-FIX-PLAN-2026-09-29）完全一致：**每批 spec + 门禁 + ocr 复审**。

## 0. 一句话

把「新拟态凸起卡片」换成「Linear 式扁平分层」设计语言，分 U1–U4 四批，每批独立可验收可回滚；U3 与已登记的 B5-2/3（ChatPanel 拆分）合并执行。

## 1. 接手先读（按序）

1. 本文档（批次定义 + 红线 + 提示词）
2. `docs/UI-REDESIGN-PROPOSAL-2026-09-30.md`（方案细节、截图拆解表、风险对策）
3. `docs/batches/B6-HYGIENE.spec.md`（spec 格式模板）+ `scripts/batch-verify.py`（门禁字段）
4. `DEVLOG.md` 顶部（项目现状与战役总账）
5. 设计素材三件套（见 §3）

## 2. 拍板记录（老板已定，不再讨论）

| # | 事项 | 决定 |
|---|---|---|
| 1 | 主窗口信息架构 | **A**：看板仍是主视图，只加左侧导航栏（不做会话优先） |
| 2 | 主题 | **深浅双主题可切**，深色为默认观感基准，浅色同步重做 |
| 3 | 图标库 | **引入 lucide-react**（唯一新依赖，MIT，tree-shake） |
| 4 | 节奏 | **U1→U4 顺序各一批**，批批验收 |
| 5 | 窗口材质 vibrancy（Mica/Acrylic） | 默认**缓做**，登记为 U4 后可选项（Tauri 实验特性，风险后置） |
| 6 | a11y 基线 | 默认**并入 U4**（focus-visible、对比度、三态组件、键盘可达） |

## 3. 设计语言基准与素材

**基准**：Linear 式暗色扁平分层——三层表面靠亮度分档 + 1px 半透明边框，无拟态阴影；
Inter 式字重纪律（400 阅读 / 500 强调 / 600 宣告，禁 700+）；唯一彩色强调只给主 CTA
与激活态；细节克制（参照截图：mono pill 徽章、相对时间、可折叠进程浮卡、diff 摘要条）。

**素材三件套**（装在 `~/.agents/skills/`，新会话自动进技能列表；若未列出，直接读文件）：

| 路径 | 用途 | 用法 |
|---|---|---|
| `~/.agents/skills/design-ref-linear/` | Linear 反向工程规范（DESIGN.md + tokens.css + design-tokens.json） | U1 写 token 前通读 DESIGN.md；tokens.css 作 `:root` 变量校准输入 |
| `~/.agents/skills/frontend-design/` | Anthropic 官方审美纪律（反 AI 味清单） | 每批动工前过一遍 §「AI 生成设计聚类」五条俗手 |
| `~/.agents/skills/design-critique/` | 5 维评分评审（逐分证据 + Keep/Fix/Quick-wins） | 每批收尾自评，报告贴对话，随 DEVLOG 归档结论 |

**优先级铁律**：项目自己的 DESIGN-SYSTEM token（U1 产出）> design-ref-linear 参照 >
frontend-design 通用审美。参照只做校准，不逐字照抄（字体栈按 wmessage 实际可用调整）。

**视觉验收**：`npm run dev` 起前端后用已装的 `web-gui-tester` 技能做黑盒截图比对
（三主题 × 主要页面矩阵，见各批验收）。skill 管手感，截图管事实。

## 4. 批次定义

### U0 — 交接文档入库（本次已完成）

PROPOSAL + 本文档 + 本 spec 入库，工作区归零。无代码。

### U1 — 设计 token 与材质体系（换肤地基，S-M）

- **范围**：`src/ui/main.css`（360 行）token 重写：三层表面 `--bg/--surface/--surface-raised`、
  1px 边框语义、圆角梯度（8/12/16）、字号阶梯（13/14/15/16）、强调色收敛（success 绿 +
  一种品牌强调，低饱和蓝紫灰可保留为品牌色）；阴影从双阴影拟态改「单层柔和投影+边框」。
- **核心手法**：`.nm-card / .nm-btn / .nm-inset / .nm-outset ...` **同名重实现**为扁平
  分层材质——类名不变、实现全换，上层组件零改动换肤。动 class 名 = 违反本批红线。
- **依赖**：`npm i lucide-react`（package.json + lock 入本批）；先铺 IconButton 基础件
  （新文件），emoji 图标（🗑✎）替换只做「顺手处」，大范围替换留 U4。
- **验收**：三主题 × 6 主要页面（看板/聊天/归档/回收站/工作区/设置）截图比对；test-fast
  全绿；双窗口主题切换回归（storage 同步机制在 `src/theme.ts`，95 行，机制不动）。
- 预算：main.css 大改 ±150 行内 + 新增 IconButton 小文件。

### U2 — 主窗口骨架：左导航 + ⌘K（M）

- **范围**：`src/App.tsx`（722 行）加左侧窄导航栏：新建任务（⌘N）/ 搜索（⌘K）/ 视图切换
  （看板/归档/回收站/工作区）/ 底部设置入口；顶部工具条相应瘦身（职能迁移，不全删）。
- ⌘K 浮层（新组件）：任务标题 + 会话标题聚合搜索，键盘上下选择 + 回车跳转。
- **红线**：KanbanBoard 三列拖拽核心交互不动；`src-tauri/` 零改动（快捷键纯前端 keydown
  实现，不碰全局注册）；双窗口事件桥不动。
- **验收**：⌘N/⌘K 键盘流可用；拖拽回归；双窗口同步回归；三主题截图。
- 预算：App.tsx ±120 行 + 新导航/浮层组件 2 个文件。

### U3 — ChatPanel 重构（M-L）⚠️ 与 B5-2/3 合并，拆两步提交

现状：`src/components/ChatPanel/ChatPanel.tsx` 1851 行（Fold/RichText/UserBubbleContent/
constants 已拆出，主文件仍巨石）。

- **U3a 拆分（行为等价）**：拆为 会话列表 / 消息列表 / 输入区 / hooks 四文件 + 补
  MsgBubble `React.memo`（流式性能项，B5-2/3 登记范围）+ 顺手清 oxlint 存量 32 warn。
  本步**只拆不换肤**，全部 ChatPanel 测试迁移绿。
- **U3b 换肤（纯表现）**：
  - 会话列表 → 左侧会话栈（标题 + 相对时间，`format.ts` 已有相对时间可复用则复用）；
  - 助手消息 → 无边框富文本卡；工具调用 → mono pill 徽章行 + 可折叠「进程 N/M」浮卡；
  - 文件变更 → 「N 个文件已更改 +X -Y」摘要条（diff 数据已有接口可接）；
  - 用户消息 → 右对齐浅底气泡；
  - 输入区 → 大圆角输入卡：附件（+）、模型选择器（读 bot-config）、权限模式显示
    （ask/yolo/strict，读配置）、圆形发送钮；斜杠命令浮层保留。
- **红线**：两步分别可回滚；消息数据流/斜杠命令语义/Tauri 命令调用零改动；流式渲染
  性能不回退（memo 前后对比留数据）。
- **验收**：ChatPanel 全部测试绿；流式性能对比；三主题截图；模拟长会话滚动不掉帧。

### U4 — 设置页 / 挂件 / 细节打磨（S-M）

- SettingsPage（1441 行，含 McpPanel 622）换新材质后调间距/分组/空态（U1 已自动换肤
  打底，本批只做布局层）；
- WidgetApp（927 行）细节对齐：圆角/边框/字号跟随新 token，三视图布局不动；
- 空态统一 `EmptyState` 组件（标题+说明+行动按钮）；
- 动效克制清单：面板滑入 150ms、按钮 100ms、无弹跳、`prefers-reduced-motion` 尊重；
- a11y 基线（拍板 §2-6）：focus-visible 全交互件、对比度过 WCAG AA、三态组件、键盘可达；
- EvolutionPanel / ConfirmMap / ArtifactBatchDialog 次级面板统一过一遍；emoji 图标
  清尾（lucide 替换收口）。
- **验收**：三主题 × 全页面矩阵终审；5 维评审终版报告；test-all 全绿。

## 5. 每批工作流（照旧，七步不许省）

```
实现 → 定向测试 → ocr review 复审（HIGH 当场修）→ 修意见 → 写批 spec
  → BATCH_SPEC=<spec> git commit（钩子自动跑 batch-verify + test-fast）
  → DEVLOG 条目
```

- ocr 复审：`ocr review`（会话级复审，`ocr session export -o x.html` 导出留档）。
  **HIGH 及以上当场修完再进 spec**；MED 记入 spec `findings`。
- spec 落 `docs/batches/<批号>.spec.md`（U1-TOKEN / U2-NAV / U3a-CHAT-SPLIT /
  U3b-CHAT-SKIN / U4-POLISH），JSON 块格式见 §6。
- 提交信息尾部标注（xxx spec），照仓库惯例。

## 6. spec 与门禁速查

**门禁机制**：`.githooks/pre-commit`（core.hooksPath 已配）——staged 触及 `src/` 或
`src-tauri/` 时强制 `BATCH_SPEC` 环境变量 → `python3 scripts/batch-verify.py --quiet`
→ 通过后跑 `scripts/test-fast.sh`。纯文档批免 batch-verify 但 test-fast 照跑。

**spec JSON 字段**（```json 块，脚本读取）：

| 字段 | 含义 |
|---|---|
| `batch_id` / `family` | 批号 / 族（UI 批用 `ui-redesign`） |
| `expected_files` | **staged 全集精确匹配**（多一个少一个都 fail；rename 对按新路径计） |
| `max_lines_added` / `max_lines_removed` | 只统计 M（修改）文件的 ±行 |
| `max_new_files_lines` | A（新增）文件总行预算 |
| `findings` | `[{file, ...}]`——ocr 复审 MED 项所在文件必须在 staged 内 |
| `assertions_min` | `{文件: 最少 assert 数}`（Rust 批用；前端批可不声明） |
| `stop_conditions` | 照抄 `["compile_failure", "gate_fail"]` |

**spec 起草后自查三条**（B6 惯例）：① expected_files 是否 staged 全集（含 spec 文件
本身！B6 踩过）② 预算是否对得上 numstat ③ findings 留空写明原因。

## 7. 环境红线（Mimosa PreToolUse 钩子，违者直接被拦）

1. **Bash 写源码会被拦**——所有 src/src-tauri/scripts 改动走 Write/Edit 工具；连
   `cat`/`wc`/`grep` 带这些路径都可能误拦，换 `find … -print0 | xargs wc` 或 Read 工具。
2. **`Command::new(变量)` 判命令注入**——Rust 侧如需改进程拉起，保持字面量 match arm
   （参考 `bot/mcp/manager.rs` 的 spawn_service + parity 测试模式）。
3. **批号模式扫描**——新增 .rs/.ts/.tsx 行禁 `P0/P1/P2-N`、`T N-N`、`NEW-X-N` 字样；
   UI 批号 U1–U4 不在禁列。写「评审」「登记项」等措辞替代。
4. commit 时偶发 `scanner_enobufs` 报错 = 扫描器内存不足，兼容性放行，不算门禁失败。

## 8. UI 特有红线（违反 = 批不验收）

- `nm-*` 类**只换实现不改名**（U1 核心杠杆；改名会打断全仓引用与测试）。
- 主题机制（`theme.ts` 三态 + 双窗口 storage 同步）、双窗口架构、事件桥、SQLite、
  Tauri 命令面：**零改动**。UI 批原则上不碰 `src-tauri/`；确需新命令须在 spec 单列
  理由并单独跑 Rust 定向测试。
- 看板拖拽（@dnd-kit 三列）、挂件悬停展开/拖动吸附等核心交互**行为不变**。
- 测试按语义查不按样式查：本仓测试断言不依赖 css 类名（此前已验证），若新断言要查
  样式，先在 spec 里说明理由。
- 每批改动后 `npx --no-install oxlint src` 不得新增 warn（存量 32 warn 仅 U3a 清）。

## 9. 基线快照（交接日）

- 分支 `main` @ `00c7fc7`，工作区干净（本文档批入库后归零）。
- test-all 1277 通过（2026-09-29 §10 验收基线）；clippy 169 warn（治理中，UI 批不新增）。
- 关键文件行数：App.tsx 722 / ChatPanel.tsx 1851 / SettingsPage.tsx 1441（McpPanel 622）/
  WidgetApp.tsx 927 / main.css 360 / theme.ts 95。
- 包管理器 npm（package-lock.json）；无 lucide-react（U1 引入）。
- 挂账项（不属本战役，勿顺手做）：B3-5 model loop 拆分、169 clippy 机械修、
  noUncheckedIndexedAccess（174 处）、KeySlot 二期按钮、Mimosa scanner_enobufs。

## 10. 开工提示词（新会话整段粘贴）

```text
执行 wmessage UI 改造战役。先读 docs/UI-REDESIGN-HANDOFF-2026-09-30.md
（批次定义/红线/门禁速查都在里面），再读 docs/UI-REDESIGN-PROPOSAL-2026-09-30.md。

硬约束：
1. 每批走七步流程：实现 → 定向测试 → ocr review 复审（HIGH 当场修）→ 修意见 →
   写 docs/batches/<批号>.spec.md → BATCH_SPEC=<spec> git commit → DEVLOG 条目。
2. 设计基准：Linear 式扁平分层；动工前读 ~/.agents/skills/design-ref-linear/DESIGN.md
   校准 token，写码时遵守 ~/.agents/skills/frontend-design/ 的反 AI 味纪律；
   项目 token 规范优先于参照素材。
3. UI 红线：nm-* 类同名重实现不改名；主题机制/双窗口/src-tauri 零改动；看板拖拽
   交互不动；oxlint 不新增 warn。
4. 每批收尾跑 ~/.agents/skills/design-critique/SKILL.md 的 5 维评审，报告贴对话。
5. 视觉验收用 web-gui-tester 截图比对（三主题 × 该批涉及页面）。
6. 本批只做 U1（token 与材质地基 + lucide-react 引入），完成并提交后停下，
   等我验收说「继续」再接 U2。
```

后续批次同提示词，把第 6 条换成对应批号即可（U3 记得说明 U3a/U3b 两步提交）。
