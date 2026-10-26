# 实机变异测试报告：V76–V96（2026-10-25 → 10-26 夜间）

对 `docs/realmachine-smoke-test-2026-10-25.md` §四 变异注入总表（V76–V96，21 条）的执行报告，接 `docs/realmachine-smoke-report-2026-10-25.md`（行为面 16 PASS + F1–F4 已修）。执行者：ZCode 代理（自动化驱动 UI + 只读取证 + 临时改源码注入）。执行时间：2026-10-25 23:00 → 2026-10-26 02:35。

| 项 | 值 |
|---|---|
| 被测版本 | 源码 HEAD `16fa2eac`；每次注入单改 1–2 处、`git checkout` 复原后重建；dev 二进制 + vite :1420 |
| 平台 | macOS（darwin 25.6.0 arm64） |
| 模型 | 主力 MiniMax-M3（anthropic）；V82/V84/V85/V86 探针用 DeepSeek V4 Flash（openai）+ 本地 mock 侧调服务器 |
| 纪律 | 一次只注一条（V89+V90、V93+V94 两批为正交可分观察的同批注入）；变异体不提交；每轮注完 `git checkout` 复原并重建；最终 `git status` clean |
| 总评 | **21 条全部有结论：12 条 FAIL 复现（判据有牙）、2 条部分成立、4 条无牙齿（行为不退化）、2 条本机不可激活、1 条注入面成立行为面不敏感。副产品：1 项产品缺口（滑动窗一次性门空消费）+ 1 起钥匙串事故全程复盘（已彻底解决）** |

---

## 一、逐条结论

| 编号 | 注入 | 卡 | 结论 | 关键证据 |
|---|---|---|---|---|
| V76 | 滑动窗候选集含 role=user | M75 | **FAIL 复现✓**（对照✓） | 同 chars=32335：变异裁 3 条（含用户口令消息），模型自述「原始请求正文被上下文压缩掉了」、`lookup_receipt("user-1")` 查不到、猜任务做偏；干净版裁 2 条、逐字复述用户原话 |
| V77 | 去掉滑动窗一次性门 | M75 | **FAIL 复现✓**（对照✓） | 同一 trace 内 `loop.tool_window.applied` 连发 **5 次**（trimmed 4→8→12→16→20 同批消息反复重裁）；干净版同场景恰好 1 次 |
| V78 | lookup_receipt 查不到返回空串 | M76 | **FAIL 复现✓**（对照✓） | 变异：模型答「返回为空」+靠自己猜原因（语义不可辨）；对照：模型逐字转告「查不到……不要凭记忆复述」并给后续选项 |
| V79 | 桩子提示参数 tool_call_id→trace_id | M76 | **部分成立** | 桩子承诺的一步直达失效——模型没按假参数调，改走无参清单 + tool_call_id 二跳恢复，答案正确（36ms 与夹具一致）。判据「调不回原文」过强：真实形态是「契约断、多跳回」，建议改卡 |
| V80 | 截断目标比例钳制放宽到 100（贴线裁） | M77 | **FAIL 复现✓** | budget 20000 + pct=100：连续三轮 `chat.history_truncated` dropped 3→5→7 逐轮爬升、`chat.summary_reused` 全场 0 次（F3 的摘要复用被贴线裁废掉）；基线（F3 复验同 budget）dropped 稳定 + 复用每轮命中 |
| V81 | 去掉「最后一条本轮输入不可砍」 | M77 | **FAIL 复现✓**（对照✓） | 14.6K 大末条 + budget 20000：变异 dropped=**5 全砍**（含本轮消息）→ Anthropic 转换层报「没有可发送的消息」硬错误；对照 dropped=4、末条保住、暗号 Q-77-SENTINEL 逐字答对。护栏是保命的 |
| V82 | 思考段盖每条 assistant | M78 | **本机不可激活** | 前提是 msgs 带 `reasoning_content`：DeepSeek 请求 `reasoning=none` 不产思考；MiniMax 走 thinking 块且 stamp 输入为空。两模型组合都无法让变异体生效，行为面无差异可测。单测侧兜 |
| V83 | Anthropic 分支塞 reasoning_content | M78 | **本机不可激活** | 同 V82 前提：msgs 无 `reasoning_content` 字段时转换分支不触发。MiniMax anthropic 全程 200 与基线无异。单测侧兜 |
| V84 | 侧调路由阈值→0（永远侧调） | M79 | **FAIL 复现✓**（对照✓） | 15K 短栈 create_task 也触发 `tool_selfcheck.sidecall`（对照同形态 15.5K 留在 in-loop 无侧调） |
| V85 | 侧调 prompt 去 {intent} | M79 | **FAIL 复现✓**（对照✓） | mock 侧调录到的 prompt 意图段为空（「用户要求：」后无内容，len 189 vs 对照 1270）；mock 规则（含意图→N，否则→Y）下 verdict=pass，对照 verdict=fail——意图是闸的承重输入，实锤 |
| V86 | 侧调失败视为通过 | M79 | **注入面成立，行为面不敏感** | 死 mock：`sidecall_failed`（连接拒绝）后静默 Some(true)→pass，闸被吞（对照 P3 同 mock 活着时 verdict=fail 会被消费）。两臂主循环请求数都是 4，回落代价在此任务形态不可分辨 |
| V87 | 注入块改回另起 system 消息 | M80 | **FAIL 复现✓** | 主请求 cache_read 出现 **21K→6.4K 回退**（前缀断在每轮变化的注入点，只剩工具+主系统可复用）；对照轮 128→7166 单调增长。M80 验收线（hit≥60%）依赖的正是这个结构 |
| V88 | 注入块拼到用户原话之后 | M80 | **无牙齿（强模型）** | 三轮记忆冲突探针全部答「红色」（按用户最新指令），M3 指令遵循压过 recency 效应；结构面翻转只有单行 diff，弱模型风险仅理论 |
| V89 | 删 create_task 的 due 示例 | M82 | **无牙齿（强模型）** | 无示例首轮即写对 `due=2026-12-31T15:00`（T 分隔、分钟补零）零拒绝。示例对弱模型的价值本机不可证伪 |
| V90 | 删并行引导规则 28 | M83 | **无牙齿（强模型）** | 两种话术（并列式/「依次读」）下 3 读全部同 turn=0 并行——M3 原生并行，规则非承重 |
| V91 | 去掉 scope:session 强制 | M84 | **FAIL 复现✓**（拆弹✓） | 无 scope 键的技能文件落盘后，**新会话 B 一说触发词立刻 `skill.start \| name=purple-storm`**（跨会话劫持，DSL 坏也照劫）。技能已删、复原后同触发词 0 路由 |
| V92 | canary_blind 恒 false | M85 | **FAIL 复现✓**（对照✓） | 假 lsof（PATH 前置、exit 0 空输出）制造真失明 + API 4763 在监听：变异 2+ 巡检周期 **0 报警**；对照立刻双 WARN（`listening_canary_failed` + `listening_sample_failed`）。顺带补完 M85 昨日未跑的反向半 |
| V93 | 删 MCP 超限主留痕 | M86 | **FAIL 复现✓** | mount.rs 的 `tool.output.over_budget`（带 `server=MiniBig`、`output_bytes=600004`）消失，只剩二级漏斗 30049/30000 事件——**原始字节数彻底无据可查**（昨日 M86 两条齐备）。对照引 M86 PASS |
| V94 | free-form 裸行与结构化并存 | M87 | **FAIL 复现✓** | write_event 旁路裸写 34 条无结构行；探针 `report` 读出 **0 条**——判据「report 条数=grep 条数」被打破。对照引 M87 PASS |
| V95 | 去掉机器人气泡限宽 | M88 | **FAIL 复现✓**（对照✓） | 长回复右边缘越过用户气泡右缘约 40px 通栏；复原后同屏右缘重新对齐 |
| V96 | 拆出去的函数挪到别的模块 | M91/M92 | **三面全✓** | 核对器半：越界搬移 → `verify_pure_move.py` exit 1「chunk 在目标文件里找不到逐字对应」，如实声明 exit 0；实机半：createNodeMaterial 越界 → **图谱页整页崩**（「Can't find variable: NODE_VERT」错误卡）；复原后图谱恢复渲染 |

**口径**：FAIL 复现 = 注入后按卡片判据观察到预期失败现象；无牙齿 = 变异已生效但行为不退化（要改卡/承认强模型覆盖）；不可激活 = 变异体前提在本机模型组合下不成立。

---

## 二、副产品发现

### F5（新，P3）：滑动窗一次性门在「跨阈值但候选不足」时被空消费

`request.rs` `compact_before_round`：`chars > TOOL_WINDOW_TRIGGER_CHARS` 即置 `tool_window_applied = true`，哪怕 `apply_tool_window` 因候选 ≤ keep_recent 返回 0（V76 场景实测：首批 4×30K 结果跨过 24K 阈值但候选仅 5 ≤ 6，flag 被消费、未裁）。后果：同 trace 内后续更大的批次**永不裁剪**（V92 之外第二个静默面）。建议：仅 `trimmed > 0` 时置 flag。本次 V76/V80 探针靠缩小夹具（3.8–4.6KB/文件）绕开；V77 的对照（每 trace 恰 1 次）也依赖此行为未触发。

### 钥匙串事故全程（已解决，含遗留决定）

变异测试每轮重编译二进制 → keyring 4.1.6（apple-native store，login keychain）的 ACL 按二进制签名记录 → 新二进制读 key 依次弹 4 次授权（MiniMax 主 key、Tavily、DeepSeek、MiniMax vendor）。事故升级链与最终状态：

1. 自签证书 `wmessage-dev`（`security add-trusted-cert -p codeSign`）+「始终允许」**不能**跨重编译生效（ACL 回退按 cdhash 记录）；`-T ""` 实测接近 deny-all，**`-A` 才是允许所有应用**（绕了两轮弯）。
2. 应用启动期的 keyring 迁移写（`keyring.rs:444 set_password(&old)`）会把当时二进制的签名写回 ACL，覆盖外部免密设置——时序上必须「旧进程死透 → 重置授权 → 新进程读钥」。
3. **最终解法（本会话工具链，已随变异一起还原）**：`WM_DEV_DISABLE_KEYRING=1` 让 `secret_service_available()` 返回 false → 走既有 PlaintextFile 后端（数据目录 0600 明文文件，预先从 keychain 读出种子）→ 应用完全不碰钥匙串，任意重编译零弹窗。
4. **遗留决定（需要你拍板）**：4 条 `wmessage.bot.v1` 钥匙串条目现在的 ACL 是 `-A`（本机任意程序可读、不再弹窗）。选项 A：保持（以后重编译/出包都不弹）；选项 B：删掉让我用应用设置页把 key 重新录一遍（应用自建条目，恢复按应用授权，正常使用只在换二进制后弹一次）。
5. 备份文件（含 key 明文）已粉碎；数据目录的明文 key 种子文件已删。

### 驱动坑（给下一轮）

- 应用重启后窗口位置会漂（360,72 / 593,61 / 353,33 轮换），且 System Events 在启动后约 20s 内间歇性「window 1 无效索引」——发送必须「front → 两次 pos 一致 → 发送 → user.message 计数递增校验 → 失败重试」（`/tmp/mutest/send-robust.sh`）。
- 重启后应用恢复的是**上次会话/首页**，「新建对话」按钮只在聊天页存在——重启后的第一组点击前必须截图确认页面。
- 工具结果与裁剪**都不跨 trace**：任何「读文件后追问」的探针必须把读写压进同一条消息（M79 的原始设计如此，V76/V79/V84 都在此处返工过）。
- 大文本驱动用 `pbcopy < 文件 + Cmd+V`（`sendfile.sh`）；`head -c` 按字节切中文会算错字数。

---

## 三、判据修正建议（改卡不改码）

| 卡/变异 | 建议 |
|---|---|
| M76/V79 | 「调不回原文」改为「桩子承诺的一步直达失效（多跳/降级可观测）」——强模型会自愈 |
| M79/V86 | in-loop 回落的可观测锚点不能落在轮数（本形态两臂同 4 请求）；建议锚「sidecall_failed 后闸是否仍能消费 verdict」的结构断言或 mock 可控 verdict |
| M80/V88 | recency 探针对 M3 无效；要么换弱模型采样，要么承认该护栏只有结构保障（单测） |
| M82/V89、M83/V90 | 示例/引导类变异对 M3 无牙齿——这两条卡的价值声明应改为「对弱模型/其他厂商的保险」，本机不可证伪 |
| M85/V92 | 反向半已可用「PATH 假 lsof」法复现（无需动系统文件），建议写进卡面替代「摘 lsof/改权限」的原方案 |
| M78/V82、V83 | 卡面补注：变异面要求模型产出 `reasoning_content`（DeepSeek 需开思考参数、MiniMax 走 thinking 块不进该字段）——本机无激活路径 |

---

## 四、环境台账

**已还原**：V76–V96 全部源码注入（最终 `git status` 仅剩未跟踪的本报告）；bot-config.json（selfcheckModelId/mock 模型条目/Local verified/echo 开关已移除，mcpServers 仅剩 builtin-playwright 停用态，provider=MiniMax-M3 anthropic，budget/pct 无覆盖）；keyring.rs 旁路开关已还原；明文 key 种子文件已删；api-enabled.flag 已删；fake lsof 目录已从启动脚本摘除；purple-storm 技能目录已删（劫持解除，实测 0 路由）；测试卡 9 张（买牛奶×4/买鸡蛋×2/季度复盘/买牛奶闸拒绝系）已软删；测试垃圾记忆 8 条已删（颜色偏好被探针改成「红色」的那条一并删除）；kc-backup.json（含 key 明文）已粉碎；sidecall mock 服务器已停。

**遗留（运行态）**：4 条 `wmessage.bot.v1` 钥匙串条目 ACL 为 `-A`（见上文遗留决定）；测试会话 20+ 个留在应用内（测试期口径：不做数据清理，可在聊天页手动移除）；`/tmp/mutest/`（夹具与脚本，系统自清）；本报告未提交（随工作区留待审阅）。

**最终验证**：干净二进制（HEAD 构建）+ 原钥匙串路径发消息「恢复OK」→ 200，1.9s。error.log 无新增 panic。

---

## 五、修复队列建议（按优先级）

| # | 严重度 | 内容 | 来源 |
|---|---|---|---|
| 1 | P3 | ~~滑动窗一次性门空消费~~ **已修 `fd43e4da`**（`trimmed > 0` 才置 `tool_window_applied`；复现测试 one_shot_gate_survives_a_threshold_crossing_without_trim 进 tool_window.rs） | §二 F5 |
| 2 | P3 | 「降级文件存储」字样出现在 keyring 读取错误文案里（`has_api_key_at` PlaintextFile 分支的报错与 System 路径共用词），排查时误导定位方向 | 本次排查实录 |
| 3 | 卡面 | 按表三修正 5 张卡的判据口径（M76/M79/M80/M82/M83 + M85 反向半做法） | §三 |
