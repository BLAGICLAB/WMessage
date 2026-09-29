# Rust Bot 全面审计计划（2026-08-27）

> 范围：`src-tauri/src` 全部 24k 行（bot* 核心 + db/migration/mutation + api_* + middleware/intent_router/tool_guard + audit/error/profile/consts + lib.rs 生命周期）+ `src-tauri/tests` + 前后端契约面。
> 与当日已完成的「工具/Skill 链路逻辑审计」（`AUDIT-BOT-PIPELINE-2026-08-27.md`，P0/P1/P2 已修完）的关系：**不重复**——那轮覆盖调度器/提示词/工具注册一致性，本轮覆盖其余全部维度，并以批次 0 做其修复的回归验证。

## 一、严重度定义

| 级别 | 定义 | 处置 |
|---|---|---|
| P0 | 确定性 bug / 数据丢失或损坏 / 安全边界可绕过 / 确定性崩溃 | 当轮立即修 |
| P1 | 逻辑分叉、口径不一、边界条件错误、资源泄漏 | 当轮修或下次迭代 |
| P2 | 健壮性、文案/注释漂移、审计空洞、测试盲区 | 排期修 |

## 二、方法学（每批统一）

1. **并行子代理分域审计**：每批 2-4 个 explore 子代理，各自带明确文件清单与「找什么问题」清单；只读。
2. **P0/P1 人工复核**：子代理发现的每条 P0/P1 必须由主代理在源码逐条复核（防幻觉行号/结论），确认后才进报告。
3. **实证优先**：能写复现用例的写用例（mock_llm / tempfile / mock_app 已有基础设施）；涉及运行时行为的查 bot.log 佐证。
4. **修复纪律**：审计与修复分批走，先出报告再按 P0→P1→P2 修；每个修复带测试，修复后跑全量（cargo test 全目标 + vitest + tsc）。
5. **留痕**：每批结果落 `docs/AUDIT-<领域>-2026-08-28.md`，修复记 DEVLOG。

## 三、批次计划

### 批次 0：基线与回归（0.5 天）
- 确认三轮修复（P0/P1/P2）无回归：`cargo test` 全目标绿、vitest 绿、tsc 零错。
- 手动冒烟清单过一遍：聊天问答、润色 Word（.NET 引擎路径）、任务卡执行（🤖）、逐步执行、/stop、技能路由命中、删除确认弹窗。
- 产出：基线记录（测试数、已装技能清单、bot.log 有无异常）。

### 批次 1：安全纵深（1.5 天，最高优先）
**文件**：`bot_fs.rs` / `bot_web.rs` / `bot_py.rs` / `api_auth.rs` / `tool_guard.rs` / `middleware.rs` / `bot.rs`（key 管理段）/ `db.rs`（降级 key 路径）/ `capabilities/` / `tauri.conf.json`
**关键问题**：
- 路径白名单：canonicalize 竞态（TOCTOU）、软链/硬链逃逸、`~` 展开边界、Windows 盘符/UNC 路径、大小写不敏感文件系统误判
- SSRF：`check_public_url` 的重定向逐跳校验是否覆盖 DNS rebinding、IPv6 映射地址（::ffff:127.0.0.1）、十进制/八进制 IP 写法、0.0.0.0、metadata 端点（169.254.169.254）
- 注入：LLM 输出进 SQL 的路径（全部走参数化？）、params.json 传参边界、文件名注入（gen_out_path basename 剥离是否覆盖所有入口）
- 密钥：keychain 失败时的降级明文 key 文件权限位、日志里会不会泄 key（truncate_for_log 覆盖全审计点？）
- 子进程：run_python 限额 bypass（fork 炸弹在 RLIMIT_AS 下表现）、dll 劫持（dotnet 工具加载路径）
- 确认弹窗：confirm id 可预测性、60s 超时边界、挂件不可见拒绝是否可绕过
- Tauri capabilities：scope 最小化复核（opener/fs/dialog 权限面）
- 产出：攻击面清单 + 每条结论带复现路径或「已核对安全」证据

### 批次 2：数据层与一致性（1 天）
**文件**：`db.rs` / `migration.rs` / `mutation.rs` / `task_out.rs` / `consts.rs`
**关键问题**：
- 事务完整性：多步写是否都包事务；-wal/-shm 拷贝迁移的正确性；迁移触发条件（json_len > db_len）的误判场景
- 并发写：前端 UI 编辑 / bot 工具 / API server / 定时调度四路写同一任务库的冲突解决（最后写赢？丢更新？）
- 迁移幂等与回滚：重复启动、迁移中断（进程被杀）后能否恢复；旧版本数据打开新版本再回退
- 回收站/归档语义一致性：deleted_at/archived 在各查询路径的过滤是否完整
- broadcast_after_mutation：三端（主窗/挂件/设置页）同步的丢事件/乱序
- 产出：并发写冲突矩阵 + 迁移场景测试清单

### 批次 3：LLM 协议与流式（1 天）
**文件**：`bot_model_loop.rs` / `bot_chat.rs` / `bot_plan.rs` / `tests/mock_llm.rs` / `tests/llm_integration.rs`
**关键问题**：
- SSE 解析：跨 chunk 断行、tool_calls 增量拼接（index 乱序/空洞）、`[DONE]` 变体、非标准换行（\r\n / 裸 \r）
- think 块拆分：嵌套/未闭合标签、流式边界切在标签中间
- 协议完整性：tool 响应与 tool_calls 配对（P1-8 占位回填的正确性回归）、消息序列非法时 API 400 的所有形态
- 上下文管理：历史截断策略（/compact 200k 字符）与模型 context window 的匹配、工具结果超长截断
- 成本：每轮请求重复发送全量历史的 token 消耗估算；Planner/Replan 额外调用的预算
- mock_llm 覆盖盘点：哪些异常响应形态（空 choices、缺 message、流式中途错误）没有测试
- 产出：协议边界用例清单 + mock 测试补齐建议

### 批次 4：HTTP API server（1 天）
**文件**：`api_server.rs` / `api_handlers.rs` / `api_auth.rs` / `api.rs`
**关键问题**：
- 绑定地址（127.0.0.1 还是 0.0.0.0）、token 认证全端点覆盖、ct_eq 时序安全复核
- 与 bot 共享状态的竞争：API 写任务库 vs UI/bot 写；broadcast 一致性
- 输入校验：各 handler 的参数边界（长度/类型/注入）、错误是否泄内部信息
- 生命周期：退出清理（api_stop_for_exit）、端口占用、重复启动
- 产出：端点 × 认证 × 校验矩阵

### 批次 5：调度与后台任务（0.5 天）
**文件**：`bot_scheduler.rs` / `lib.rs`（setup/退出清理）/ `exec_steps.rs`（定时与逐步的交互）
**关键问题**：
- 定时任务触发可靠性：错过触发时间的补偿、应用休眠/唤醒、时区与夏令时
- 后台执行（interactive=false）全链路：不弹窗、不推流、确认默认拒绝——逐工具核对（有无遗漏的工具仍尝试弹窗）
- 退出清理顺序：kill Python 子进程 → 技能终止 → API 停 → DB flush，顺序是否安全
- 产出：后台执行工具行为矩阵 + 退出时序图

### 批次 6：跨平台与资源生命周期（0.5 天）
**文件**：`bot_py.rs`（dotnet/python 探测与限额）/ `audit.rs`（日志轮转）/ `lib.rs` / `build.rs` / `tauri.conf.json`
**关键问题**：
- Windows 特有：Job Object 限额边界、路径长度、杀毒软件干扰（数据目录探针已修，回归确认）、WebView2 缺失
- 绿色包：dotnet/ 工具随包发布的完整性检查（缺失时静默回退 Python 是否被监控到）
- 资源泄漏：临时目录清扫（py-runs 1h）、日志轮转 5MB、skill_runs 表只进不出的内存增长
- 探测缓存失效：python/dotnet 中途被卸载后的行为
- 产出：平台差异清单 + 泄漏点清单

### 批次 7：前后端契约（0.5 天）
**文件**：`src/`（ChatPanel/WidgetApp/SettingsPage 等）↔ `src-tauri/src` 命令与事件
**关键问题**：
- invoke 参数名/类型与 Rust 命令签名逐一对账（serde rename、camelCase 转换）
- 事件名与 payload 结构对账（bot-chat-delta / bot-tool* / bot-confirm / tasks-updated / bot-skill-failed）
- 错误展示：CommandError code → 前端处理的映射完整性（有无 code 落到默认分支吞掉）
- 产出：契约对照表（命令/事件 × 参数 × 双方一致 ✅/❌）

### 批次 8：测试体系与防回归（0.5 天）
**文件**：`src-tauri/tests` / 各模块 #[cfg(test)] / `tests-audit/` 审计脚本
**关键问题**：
- 覆盖盲区盘点：生产 async 路径（run_skill_scheduler 零 e2e）、全局状态测试的并行污染（SKILL_RUNS / PENDING / STOP_REGISTRY / PY_CACHE 共享）、mock 与真实实现 drift（同步镜像 run_dsl_loop_sync vs 生产 async 循环）
- 测试质量：断言强度（只测存在不测内容）、sleep 依赖的脆弱测试
- tests-audit 脚本有效性：行数上限/结构断言是否跟得上重构
- 产出：盲区清单 + 补测试优先级建议

## 四、总产出物

1. 每批一份 `docs/AUDIT-<领域>-2026-08-28.md`（发现 + 证据 + 级别）
2. 汇总 `docs/AUDIT-SUMMARY-2026-08-28.md`：全维度 P0/P1/P2 清单 + 修复排期建议
3. 每批修复后 DEVLOG 记录 + 测试数基线对比

## 五、工作量与顺序建议

总估 6 天（可按优先级裁剪）：**批次 0 → 1（安全）→ 2（数据）→ 3（LLM）→ 4（API）→ 5/6/7/8 可并行**。
安全与数据两批不建议裁——它们是用户数据和系统边界的最后防线；LLM 协议批直接影响机器人可靠性观感；批次 5-8 可视前几批发现量调整深度。
