# Phase 3 分拣账本：medium bug+security（2026-09-26 起）

> 基线：fullscan medium 非 vendor 482 条，其中 bug 190 + security 25 = **215 条进本分拣**；
> maintainability 156 / performance 42 / test 27 / other 34 / documentation 7 / style 1
> = 267 条按计划「走 ponytail 判定，多数 wontfix」→ **整体登记 wontfix-for-now**（Phase 4 前如需翻案逐条提）。
> 分拣格式：`[编号] 文件:行 | 处置（FIX/SKIP+理由）`。分拣未完成项持续补充。

## security（25 条，2026-09-26 分拣完毕）

| # | 位置 | 处置 |
|---|---|---|
| S0 | .gitignore:65 | **FIX（P3-SEC-1）** sibling 凭据模式补齐 |
| S1 | capabilities/default.json:5 | SKIP——权限模型红线；C2a-Q1 已登记 wontfix（widget 共用=产品决策） |
| S2 | api_handlers/body.rs:0 | **FIX（P3-SEC-1）** Transfer-Encoding 即拒 |
| S3 | api_handlers/body.rs:54 | SKIP——错误路径断连即可；drain 反扩 slowloris 面 |
| S4 | api_handlers/types.rs:5 | SKIP——API 响应形状契约 |
| S5 | api_handlers/types.rs:18 | SKIP——同上（空串 token 理论态） |
| S6 | api_handlers/util.rs:112 | SKIP——下游已有 validate（check_export_path 等） |
| S7 | bot.rs:110 | SKIP——登记待读（add_allowed_dir 审计面，低优） |
| S8 | bot/dispatch.rs:174 | SKIP——本地审计文件，用户即数据属主；redact 破坏诊断价值 |
| S9 | bot_skills/files.rs:144 | **FP**——canonical 后 contains（C2b 设计），非 raw |
| S10 | bot_slash.rs:166 | **FP**——truncate_for_log=escape_for_log 别名（含控制字符全集） |
| S11 | bot_web.rs:963 | SKIP——主形态（纯数字/hex/octal）已覆盖；混合进制（0x7f.1）登记残余 |
| S12/S13 | db/mod.rs:146/:213 | SKIP——DDL 标识符来自内部字面量数组，非用户输入 |
| S14 | db/paths.rs:12 | SKIP——data_dir==db_dir 设计决策（wontfix-pending-product） |
| S15 | migration/ops.rs:131 | SKIP——C1b-4 策略（Windows defer） |
| S16 | ocr.rs:37 | **FIX（P3-SEC-1）** scheme 小写归一 |
| S17 | paths.rs:330 | **FIX（P3-SEC-1）** inline 与 audit 全集对齐（parity 期望更新） |
| S18 | py/document.rs:847 | SKIP——桌面应用无 sandbox 语境 |
| S19 | py/document.rs:1172 | SKIP——抽样调用已 escape_for_log；全量审计登记低优 |
| S20 | py/runtime.rs:455 | **FIX→回退登记**——硬失败实测打断 py_exec（sandbox EINVAL）；pending-environment-decision |
| S21 | tauri.conf.json:21 | SKIP——CSP unsafe-inline 改动需前端渲染全量验证（wontfix-pending-product） |
| S22 | ChatPanel/types.ts:21 | SKIP——React 默认转义；MarkdownText 已在 FE-04 处理 |
| S23 | lib/openTarget.ts:88 | SKIP——服务端 path_openable_in fail-closed 才是边界 |
| S24 | vite.config.ts:18 | SKIP——dev 工具配置 |

## bug（190 条）

### 已修（P3-BUG-1，commit 见 §0）

- B20 handlers.rs:39 json_ok 序列化失败静默 2xx+`{}` → **FIX**：显式 500 + 错误体
- B24 ratelimit.rs:47 open/writeln 双静默 → **FIX**：eprintln 可见化
- B27 util.rs now_ms 时钟异常取 0 无诊断 → **FIX**：留痕后取 0（行为不变）
- B29 eval_run.rs flag 值吃 flag → **FIX**：arg_value 拒绝（exit 2 对齐 CLI 风格，OCR r1 采纳）
- B40 bot_chat.rs:1432 register 早于 ChatGuard → **FIX**：移到 acquire 成功后（早退不泄漏）
- B42 bot_model_loop.rs:680/:692 重试 sleep 不响应 /stop → **FIX**：sleep 前 stopped() 早退 ×2（镜像主循环口径）

### 已修（P3-BUG-2，commit 见 §0）

- B49 parse.rs:74 单引号 scalar → **FIX**：trim_matches 双臂
- B50 parse.rs:319 编号空洞静默 → **FIX**：1 起连续递增检查
- B56/B57 bot_slash.rs:322/:404/:432 字节边界 panic → **FIX**：chars().take(8)（:322 逗号修复当批自愈）
- B60 bot_sessions.rs:129 rename 静默 Ok → **FIX**：rows==0 → InvalidArgument
- B86 synthetic window_days → **stale**：validate() 入口已拒（EV-3b-F）


### 分拣（续批处理；本批未覆盖）

- B0 .cargo mingw 跨平台 guard — SKIP（CI 工具链问题，非应用 bug）
- B1/B2 pre-commit 路径健壮性 — SKIP（hook 基建，HOOK-1 族）
- B3 index.html system 主题不跟随 — DEFER（产品行为：跟随需订阅链路，涉 theme 架构）
- B4/B5 ci-guard stderr/exit code — SKIP（CI 脚本卫生）
- B6 fetch_ocr_models trap — SKIP（同上）
- B7/B8 install-hooks 静默/未验证 — SKIP（同上）
- B9/B10 publish-docx rm-rf/完整性 — SKIP（发布脚本，非应用运行时）
- B11 sync-version semver 校验 — DEFER（脚本健壮性，低频）
- B12 test-fast CJK \b 边界 — SKIP（平台差异，登记）
- B13 test-fast config-only 全跑 vitest — DEFER（门禁性能，非正确性）
- B14 dotnet Heading1 样式 — SKIP（dotnet 面，独立评估）
- B15 pbtest.rs 结果未检查 — SKIP（example 二进制）
- B16 api_auth write_enabled_flag 静默 — DEFER（与 #15/#16 同语义域，登记）
- B17 service_present None 处理 — DEFER（语义设计域，登记）
- B18 start_api 持锁注释 — DEFER（登记）
- B22 max_order f64 2^53 — DEFER（量级理论，登记）
- B23 update_task deleted=false 不清 archived — DEFER（API 语义决策，登记）
- B25 ratelimit rotate TOCTOU — SKIP（与 AP-03 同域，rotation 容错设计）
- B26 SSE lastEventId 无 id: 字段 — DEFER（前端 EventSource 兼容域，登记）
- B28 after_change emit 顺序 — SKIP（理论 panic 顺序，emit_fn 不 panic 契约）
- B30 audit_log let _ 布尔丢弃 — SKIP（helper 内已有 eprintln，可见性已达成）
- B31 bot_set_config 空 key 静默 — DEFER（UX 语义，登记）
- B32 load_config 迁移错误吞 — SKIP（fallback 到 parse 已是链路设计）
- B33 keyring has/read TOCTOU — SKIP（keyring 面单进程）
- B34 migrate 重试无退避 — DEFER（登记）
- B35 minimaxi needle — SKIP（MiniMax 官方域名实为 minimaxi.com，needle 正确，OCR FP）
- B36 migrate_config_file 跨写者 race — SKIP（CONFIG_WRITE_LOCK 已在 NEW-1345 前后覆盖写路径，迁移钩子 try_lock 让路设计）
- B37 anthropic 空 id unwrap_or("") — DEFER（登记）
- B38 refusal-only 塌空 — DEFER（语义决策：refusal 文案进 reply，登记）
- B39 build_memory_block 吞错 — SKIP（设计注释「绝不弄挂主对话」明示，已有 DEBUG 日志）
- B41 grep 二进制检测 8KiB — DEFER（窗口扩大成本，登记）
- B43+ — 续批分拣。

（分拣进行中——按文件簇推进，处置追加于此。未列出的 = 尚未分拣。）

## 统计

- security：25/25 分拣完毕（5 FIX / 1 FIX→回退登记 / 3 FP / 16 SKIP）
- bug：0/190（待续）
