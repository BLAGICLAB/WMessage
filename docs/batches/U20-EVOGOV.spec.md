# Batch Spec: U20-EVOGOV

## 目的

自进化系统治理归一：决策板 toggle ON 的批准从此真实落库生效（此前只登记
pending ChangeRecord、无任何执行器，板上决策与实际生效双轨脱节）；新增
`evolution.applyPolicy` 治理开关把「自动生效 / 需我确认」二档收拢到一条管线；
决策板冒烟三件套（立即反思、四指标、行内决策徽标）让管线饿着也能人工转起来。
默认档零变化：applyPolicy 缺字段/非法值 = auto = 改前行为。

- **W1 人工批准执行器**：`toggle_inner` ON 路径三段式——①锁内 dedup/建
  ChangeRecord(pending) + 提案晋升（登记即落盘）；②锁外 `apply_one` 落
  lesson（`evo:<proposal_id>` tags[0] 持久幂等，重复批零副作用）+ applied.jsonl
  留痕 + 审计；③锁内 CR 合法流转 pending→Active（瞬时走完 Shadowing→
  ShadowPassed→Approved，同 auto_applied_from_proposal，不裸写绕状态机；
  approval_source 保持 HumanApproved，仅状态变化才 rewrite）。ConflictRefused
  （防劫持闸）走 Warn 审计 + 面板错误提示，CR 保持 pending 可重试/停用。
  toggle dedup 口径加 Active——已生效提案重复拨 ON 不新开 CR 行。非 policy 层
  （Prompt/ToolSchema/SkillHint）维持旧口径只登记 pending；auto 轨
  （apply_from_consolidation）零改动。嵌入在持锁前算（同 auto 轨纪律）。
- **W2 applyPolicy 治理归一**：新 `evolution/policy.rs`——轻读取器
  （缺文件/缺块/缺字段/非法值/JSON 坏 = auto，低频 stderr 留痕）+ RMW 写入器
  （自有锁 + atomic_write；文件缺失从 `{}` 起步；解析失败拒绝写防覆盖坏文件）。
  `post_consolidation` 尾部按 `auto_apply_allowed` 分流：confirm 档不再调
  apply_from_consolidation，达门槛提案全留候选池（write_proposals/emit 不变）
  并落 `evolution.apply_deferred` Info 审计。设置页自进化头部二档 radiogroup
  （同记忆三档先例，点档即时落盘，nm-inset/nm-outset 总闸样式对齐）。
- **W3 决策板冒烟**：空状态「立即反思」按钮接 `memory_consolidate_now`
  （结果提示 merged/distilled/contradictions）；新命令 `evolution_metrics`
  薄壳包 `observe::compute_metrics`（30 天窗口，同 observe_run 默认；空 jsonl
  零值）；行内决策徽标「已自动生效/待你决策/你已启用」（active+auto_applied >
  human_approved > pooled 优先级）+ 头部四指标条（读失败静默隐藏）。
- **修出一个存量 bug**：toggle 新建 CR 只 append 落盘、不进内存 vec，W1 的
  policy 块随后整文件重写 changes.jsonl 会把刚落的 CR 整行清掉——补
  `changes.push(cr.clone())` 同步内存视图（集成测试实测抓到）。

## 关键设计决策

- **toggle_inner 泛型化**（`AppHandle` → `AppHandle<R: Runtime>`）：Tauri 命令
  面不变（wrapper 恒 Wry），内核可被 MockRuntime 集成测试直调（run_extract_with
  先例，`#[doc(hidden)] pub`）。
- **post_consolidation 测试拆解**：字面入口依赖 `emit::APP_HANDLE`
  （OnceLock<AppHandle<Wry>>，测试进程不可注册），端到端按其真实构成拆三段
  覆盖：①ops→derive_proposals（真启发式）②write_proposals（真写盘）
  ③policy::auto_apply_allowed（生产同一函数读真 bot-config.json）；apply 段由
  toggle 执行器真跑。三分支接线本体 3 行，路径全覆盖。
- **集成测试单用例叙事**：nextest 每测试一进程，共享 deps 目录下 evolution
  jsonl 是整文件重写，两进程并发互踩必丢条目（lib 内首版实测「proposal 不
  存在」）——凡触碰共享 evolution 三 jsonl 的断言全部合并进 tests/evolution_gov.rs
  一个用例（同进程天然串行），lib 只留纯函数单测；条目 id u20-gov 前缀 +
  用毕清理（承 U19 实录）。confirm 谓词断言 set→assert 之间对 U15 门禁用例
  整文件覆写共享 bot-config.json 的竞态用幂等 set 重试 3 次对冲。
- **PolicyEntry 不加 category 字段**：ProposalEntry 加字段会打断 4 处存量
  测试的全字段字面量构造（违背「存量零改动」红线），改用 layer↔category
  双射反推（derive_layer 是单射；Parameter/Code 两层无对应返 None 跳过落库）。

## 验收数字（锚点 = 改前行为）

- **存量回归锁**：evolution 模块测试基线 283 条（git worktree 干净检出计数）
  零改动通过；全量 1391 nextest + 401 vitest + 4 pytest 审计全绿；
  test-fast.sh 11s / test-all.sh 86s 双门禁 exit 0。
- **新增 Rust 测试 11**：policy.rs 9（读矩阵 4 / None 句柄谓词 1 / 写路径 4）+
  panel 2（entry_to_proposal 双射还原 / mark_human_applied 合法流转幂等）。
  集成 1：confirm 档端到端——真 ops（Contradiction）→ derive → 入池 Pooled →
  toggle ON → `evo:<pid>` lesson 落 mem_items（kind=lesson/source=system）→
  applied.jsonl 留痕 → CR Active+HumanApproved → 重复 toggle 一条 lesson /
  一行 CR / 一条留痕（幂等 100%）→ 非 policy 层不落库 → 防劫持拒写
  （cos=1.0 撞车 → Err + CR pending + 原行不劫持）→ 3 连跑稳定。
- **前端 vitest 5 新用例**：策略二档点选落盘 / 后端 confirm 读回选中 /
  三徽标 / 空状态立即反思结果提示 / 四指标条渲染与读失败隐藏；
  EvolutionPanel 22/22 绿（存量 13 条零改动）。
- **默认档零变化**：post_consolidation 在 applyPolicy 缺字段时走原路径
  （auto_apply_allowed(None/未配置)=true）；老配置零影响。

## ocr 复审处置记录（R1 单轮：25 条 3H/7M/15L，json 于 docs/OCR-CODE-REVIEW-2026-10-04-u20.json）

- **H 修 2 + 段化重构**：④ apply 全家桶（open_db + DB_WRITE_LOCK + 全表查重
  + 写入 + 审计）压在 EVOLUTION_STORE_LOCK 内 → toggle ON 改三段式——锁内
  登记（CR pending + 提案晋升即落盘）→ **锁外** apply_one + applied 留痕 +
  审计（对齐 auto 轨「DB 不进 store 锁 / 锁内不夹审计 IO」双纪律）→ 锁内 CR
  流转（重新 load 定位，仅状态真正变化才 rewrite，⑤ 的 no-op 写放大顺手消掉）；
  ⑩ onApplyPolicyChange 缺 in-flight guard（连点乱序回滚）→ policyBusy 状态 +
  按钮禁用 + 忽略并发点档。
- **H 部分采纳 1（③）**：段化后落库失败的盘面显式为「CR pending + entry
  Promoted」；「状态不一致」半句驳回附证伪——ProposalStatus::Promoted 的语义
  即「已晋升（已创建 ChangeRecord）」（candidate/entry.rs），此时 CR 已存在
  （pending），生效性由 CR.status 承载而非 proposal.status；错误文案列出的
  重试（dedup 含 pending 命中）/ 停用（OFF 移 pending）/ 删除三路全部可达。
- **M 修 2**：⑥ evolution_get_apply_policy 返回裸 String → 类型化 ApplyPolicy
  （enum 补 serde rename_all = snake_case，Tauri 线上字面量不变）；⑯ 集成测试
  取 DB_WRITE_LOCK 缺 poison 留痕 → 统一 db_write_lock() helper（C3-1 约定）。
- **M 驳回 3（附证伪）**：⑤ 预计算嵌入「可能过期」——suggestion_text 在提案
  生命周期内不可变（无任何写路径改 entry.suggestion_text），预读值恒等于锁内
  权威值，唯一窗口是 entry 被删且锁内报「不存在」，代价一次白算嵌入；⑭ 测试
  set→assert 与 U15 门禁用例整文件覆写共享 bot-config.json 的 TOCTOU——共享
  目录固有权衡，幂等 set 重试 3 次已把窗口压到可忽略，彻底解 = 跨进程文件锁
  = paths.rs 留档 B3 方案（「等实锤再上」，本批不上新机制）；⑮ 中途 panic
  残留共享文件——开场清理前移已兜底（U19 实录口径），config 残留无消费方受
  影响（读侧缺字段 = auto 默认）。
- **L 修 5**：⑬ aria 前缀从 label 派生（防漂移）；⑱ Box::leak 补注释；
  ⑲ 点档前清旧 error/info banner；⑳ 指标条「近N天」取
  observation_window_days；㉔ dedup-Active 路径 no-op rewrite（随段化修）。
- **L 驳回/登记不修 9**（附证伪）：⑦ toggle_inner pub+doc(hidden)——集成
  测试必要面，run_extract_with 先例；⑧ 3 处 CR clone——小结构 + toggle 低频；
  ⑨ entry_to_proposal 的 None 臂——Parameter/Code 到不了调用点，防御纵深 +
  明确错误文案；⑪ decisionBadge O(n×m)——个位数提案 × 十几 CR 微秒级；
  ⑫ 两个 mount invoke 串行——本地 IPC 微秒级，各自独立 catch 语义更稳；
  ⑰ 防劫持段引擎不可用静默跳过——有意设计（U19 同口径，eprintln 留痕，
  不阻塞无模型环境）；㉑ 反思成功 info 与 refresh 失败 error——两 banner
  独立渲染并存不覆盖，信息完整；㉒ now_ms 跨段复用命名——纯美感；
  ㉓ 未断言精确派生文案——相等性已由 lesson.content == suggestion.text 锁定，
  derive 文案归属 derive.rs 自有测试。
- **不适用 3**：model-meta-service/main.py（hash 颜色、upsert 200 语义、
  fallback 键覆盖）——untracked 未入库、U12 已拍板废弃的独立 Python 服务
  目录（U15/U16/U18/U19 同口径），不随本批处置。

## 红线核对

- 默认行为零变化：applyPolicy 缺字段/非法值/读失败 = auto；post_consolidation
  auto 档路径逐行为等价（write_proposals / emit / apply_from_consolidation
  调用顺序不变）；auto 轨（High/Medium MemoryHint 自动落库）零改动。
- 状态机不裸写：CR pending→Active 经 transition 合法流转（瞬时中间态），
  approval_source 全程 HumanApproved。
- 不做：derive 启发式与 occurrence_count 语义（U21）、shadow/AB/灰度接线
  （休眠件留档）、删除休眠件、任何第二次 LLM 调用。
- SQL 全参数绑定（本批无新 SQL；mem_items 读写全走 store 层参数绑定接口）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "U20-EVOGOV",
  "family": "evolution-governance",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/U20-EVOGOV.spec.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/src/evolution/mod.rs",
    "src-tauri/src/evolution/panel/commands.rs",
    "src-tauri/src/evolution/policy.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/tests/evolution_gov.rs",
    "src/components/EvolutionPanel/EvolutionPanel.test.tsx",
    "src/components/EvolutionPanel/EvolutionPanel.tsx",
    "src/components/EvolutionPanel/types.ts"
  ],
  "max_lines_added": 800,
  "max_lines_removed": 60,
  "max_new_files_lines": 900,
  "findings": [
    { "file": "src-tauri/src/evolution/policy.rs", "note": "applyPolicy 轻读取器（缺什么都是 auto=现状）+ RMW 写入器（自有锁+atomic_write+坏文件拒绝写）；Tauri 命令薄壳在 panel/commands.rs" },
    { "file": "src-tauri/src/evolution/panel/commands.rs", "note": "W1 人工批准执行器：policy 层 toggle ON 即 apply_one 落库（幂等）+ applied 留痕 + CR 合法流转 Active；dedup 口径加 Active；嵌入锁外预计算；ConflictRefused 面板错误提示；CR 内存视图与盘同步（修存量 clobber bug）" },
    { "file": "src-tauri/tests/evolution_gov.rs", "note": "confirm 档端到端单用例叙事（nextest 每测试一进程 × 整文件重写 jsonl 的互踩对冲）；post_consolidation 按 emit OnceLock 边界拆三段真路径覆盖；u20-gov 前缀 + 用毕清理" },
    { "file": "src/components/EvolutionPanel/EvolutionPanel.tsx", "note": "头部应用策略二档 radiogroup（点档即时落盘失败回滚）+ 空状态立即反思 + 四指标条 + 行内决策徽标" }
  ],
  "stop_conditions": ["compile_failure", "gate_fail", "existing_tests_modified"]
}
```

## 验证命令

```
cargo nextest run --test evolution_gov                                    # 端到端 1 绿（3 连跑稳定）
cargo nextest run -E 'test(evolution) or test(policy)'                    # evolution 294 绿（存量 283 零改动）
bash scripts/test-fast.sh && bash scripts/test-all.sh                     # 双门禁全绿
python3 scripts/batch-verify.py docs/batches/U20-EVOGOV.spec.md           # 批校验过
```
