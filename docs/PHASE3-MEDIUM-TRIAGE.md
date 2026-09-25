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

（分拣进行中——按文件簇推进，处置追加于此。未列出的 = 尚未分拣。）

## 统计

- security：25/25 分拣完毕（5 FIX / 1 FIX→回退登记 / 3 FP / 16 SKIP）
- bug：0/190（待续）
