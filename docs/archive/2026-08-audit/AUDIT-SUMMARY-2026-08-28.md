# WMessage Rust Bot 全面审计 · 总汇总（2026-08-28 计划，批次 0–8 全部完成）

> 计划：`docs/AUDIT-PLAN-BOT-2026-08-27.md`。范围：`src-tauri/src` 全部 ~24k 行 +
> `src-tauri/tests` + 前后端契约面 + 测试体系。
> 方法：每批 2-4 路并行 explore 子代理分域扫描，**全部 P0/P1 由主代理逐条源码复核**，
> 修复带回归测试，修复后跑全量（cargo test 全目标 + vitest + tsc，批次 8 起加
> tests-audit pytest 门禁）。

## 总量

| 级别 | 总数 | 已修 | 记录/排期 |
|---|---|---|---|
| P0（确定性 bug / 数据损坏 / 安全破口） | **5** | 5 ✅ | 0 |
| P1（逻辑分叉 / 口径不一 / 资源泄漏 / 门禁失效） | **31** | 30 ✅ | 1（API slowloris，需换 HTTP 栈） |
| P2（健壮性 / 测试盲区 / 漂移） | 60+ | 大部分已修 | 见「未修排期项」 |

## P0 清单（全部已修）

| # | 批次 | 问题 | 修复 |
|---|---|---|---|
| SEC-P0-1 | 1 安全 | fetch_url 重定向逐跳校验是死代码（reqwest 默认自动跟随，手工 3xx 校验永不触发）→ SSRF 可打 127.0.0.1/169.254.169.254 | redirect(Policy::none()) 激活逐跳校验 + 回归测试 |
| SEC-P0-2 | 1 安全 | create_task/edit_task 的 files 参数零校验 + 绑定目录并入白名单不过滤已删任务 → 提示注入把主目录种进白名单，strict 模式失效 | 模型来源 files 仅放行 AI_Gen_Files 内文件且强制 isDir=false；过滤已删任务 |
| B2-P0 | 2 数据 | data.json 迁移计数误判漏迁 + 残留 json 复活已硬删任务 | 集合差判定 + 迁移后一律退役为 data.json.migrated |
| P0-1 | 3 LLM | SKILL_RUNS 按技能名为键，同名 Skill 跨会话顶号 → 先跑一方的熔断/回滚/收尾安全闸全失 | start_skill 拒绝跨会话同名活跃技能 + 单测 |
| P0-2 | 3 LLM | 流式事件 payload 无 sessionId，并行会话输出串台 | emit_stream 统一注入 sessionId + 前端按会话过滤 |

## P1 清单（30 已修 / 1 记录残留）

**批次 1 安全（7）**：grep_files 软链逃逸；SSRF 地址段遗漏（::ffff: 映射 / CGNAT）；
前端直达命令零校验（open_file_path/delete_bound_file/export 任意路径）；明文 key 残留
（keychain 恢复后迁回删文件）；fetch 内存无流式上限（2MB 截断）；日志注入漏网
（全量 escape_for_log）；attach_images 无白名单。

**批次 2 数据（4）**：copy_legacy_db 半拷贝无恢复（临时文件 + rename）；
bot_scheduler 三处写库零广播；锁外写者纳入 DB_WRITE_LOCK；WidgetApp 空列表守卫
（删光后挂件永久旧数据 + 复活已删任务）。

**批次 3 LLM（6）**：干净 EOF 残缺 tool_calls 被当完整回复执行（saw_done_or_finish
防线）；UTF-8 跨 chunk U+FFFD（drain_sse_lines 字节缓冲切行）；200 流内错误载荷静默吞
（ParsedChunk.error 显式冒出）；429/5xx 零重试（MAX_LLM_ATTEMPTS=2 + 退避 + 审计）；
聊天主路径无上下文预算（10 万字符截断）；历史图片每轮重复 base64 内联（限最后 3 条）。

**批次 4 API（2 修 + 1 记录）**：API 写路径 RMW 无锁（API_RMW_LOCK 串行化）；
死 SSE 连接占位（Weak 存活令牌 + 注册前收割）。**残留**：accept 循环级 slowloris
（tiny_http 不暴露 per-connection 读超时，彻底修需换 HTTP 栈；仅回环面，DoS 成立
但过不了鉴权）。

**批次 5 调度（3）**：调度循环 spawn 后串行 await 堵死全线（并发化 + 单任务 30min
超时）；后台定时任务仍弹原生文件框（bind_file/extract_document 无人在场永久阻塞）；
退出对在途模型循环零取消（stop_all_executions + 2s drain）。

**批次 6 平台（4）**：probe_dir 把 .app 包内目录当便携数据目录（数据库写进 app 包）；
构建机绝对路径烧进发布二进制（cfg(debug_assertions)）；绿色包「免装 .NET」未达成
（随包 apphost exe 优先直跑）；macOS 崩溃路径 Python 孤儿永久驻留（父进程看门狗）。

**批次 7 契约（1）**：挂件切模型漏传 permMode，bot_set_config 全量覆写 +
serde(default) 静默把授权模式重置回 ask（补透传 + 回归测试）。

**批次 8 测试（3）**：tests-audit 门禁红（旧签名字面量 FAIL 卡死 pre-push，改正则）；
SKILL_RUNS 并行测试实锤交错（SKILL_RUNS_TEST_LOCK）；stop_all 全局广播打断并行测试
（STOP_TEST_LOCK）。

## 未修排期项（按建议优先级）

1. **整行覆盖的 lost-update**（批次 2，最重要的架构项）：upsert 是 19 列全量替换 +
   `updated_at >=` 守卫，但所有写者都先刷新时间戳 → 守卫对「读旧快照→整行写回」
   失效。修复方向：字段级合并写入或写前重读比对，单独立项。
2. **生产 run_skill_scheduler 真 e2e**（批次 8 P2-4）：run_dsl_loop_sync 镜像照不到
   确认/审计/持久化/回滚窗口/Done 收尾——P0-1/P0-5 修复密集区恰在镜像外。
   fixtures/minimax-ppt 现成素材，约半天。
3. **run_model_loop 主体零集成覆盖**（批次 3 T-3）：~440 行 HTTP 错误包装/流中断/
   工具编排要 AppHandle。建议把 HTTP 层抽成可注入 base_url/client 的纯 async 函数后补。
4. **API slowloris 换 HTTP 栈**（批次 4 P1-1 残留）：立项评估 axum/actix 替换 tiny_http。
5. ~~recurring 补跑时效窗口~~（批次 5 F3）**已定版已修**：2026-09-02 拍板 2h 窗口，
   超窗 occurrence 跳过不补跑（classify_due + 6 个单测，lib 473 → 479）。
6. **绿色包 .NET self-contained 打包**（批次 6 G3，老板打包动作，代码已铺好）：
   `dotnet publish -r win-x64 --self-contained`。
7. **安全记录项**（批次 1）：DNS TOCTOU（需 resolve 钉 IP）、LLM base_url 零校验
   （建议 https 警告）、.NET dll 无哈希/签名校验（发版流水线补）。
8. **前端盲区**（批次 8 P2-8）：KanbanBoard.spliceMove（需先导出）> MigrationPanel >
   ArchivePage 过滤 > theme.ts > MarkdownText 正则。
9. **弱断言清理**（批次 8 P2-6）：6 处内联复刻/恒等断言（migration.rs:1525/1711/1888/
   1928/2111、db.rs:2131/2036/2239/2376、llm_integration.rs:182、skill_e2e.rs:205）。
10. **其他记录**：死命令 bind_file/db_merge（批次 7）；bot_stop/bot_confirm_response
    返回 () 失败不可见（批次 7）；审计 append 失败只 eprintln（批次 6 G7）；
    SchedGuard/ExecGuard 双守卫语义分裂（批次 5 F10，架构项）；sched_last 跑前记崩溃
    窗口（批次 5 F4，取舍可接受）；孙进程 setsid 逃逸（批次 6 G9，非沙箱已知取舍）。

## 测试基线对比（批次 0 基线 → 批次 8 收口）

| 套件 | 批次 1 基线 | 最终 | 变化 |
|---|---|---|---|
| cargo test --lib | 428 | **473** | +45（各批修复的回归测试） |
| cargo 集成（llm_integration/skill_e2e 等） | 29 | **20+8+8** | mock 保真度改造后重排 |
| vitest | 110 | **132** | +22 |
| tsc | 零错 | 零错 | — |
| tests-audit pytest | —（门禁红未发现） | **23 过 / 0 FAIL / 2 skip** | 批次 8 修复并纳入验证 |

## 各批报告索引

| 批次 | 报告 | 结论 |
|---|---|---|
| 0 基线与回归 | （无报告，验证步骤） | 三轮修复无回归 |
| 1 安全纵深 | `AUDIT-SECURITY-2026-08-27.md` | P0×2 + P1×7 全修 |
| 2 数据层 | `AUDIT-DATA-2026-08-28.md` | P0×1 + P1×4 全修 |
| 3 LLM 协议 | `AUDIT-LLM-2026-08-28.md` | P0×2 + P1×6 全修 |
| 4 HTTP API | `AUDIT-API-2026-08-28.md` | 无 P0；P1×2 修 + slowloris 残留 |
| 5 调度后台 | `AUDIT-SCHED-2026-08-28.md` | 无 P0；P1×3 全修 |
| 6 平台资源 | `AUDIT-PLATFORM-2026-08-28.md` | 无 P0；P1×4 全修（打包侧 1 项待老板） |
| 7 前后端契约 | `AUDIT-CONTRACT-2026-08-28.md` | 无 P0；P1×1 修 |
| 8 测试体系 | `AUDIT-TESTS-2026-08-28.md` | 无 P0；P1×3 全修（门禁转绿） |
