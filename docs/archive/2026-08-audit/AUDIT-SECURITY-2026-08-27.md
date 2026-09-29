# Rust Bot 安全纵深审计报告（2026-08-27，批次 1）

> 范围：文件访问边界 / 网络 SSRF / 密钥与子进程 / Tauri 权限面与确认机制。
> 方法：4 路并行子代理 + P0/P1 全部人工源码复核。基线：cargo 428+29 绿、vitest 110 绿、tsc 零错。

## P0（✅ 已修）

### SEC-P0-1 fetch_url 重定向逐跳校验是死代码 → SSRF 可绕过
`bot_web.rs:17-29` `http_client()` 未禁用 reqwest 自动重定向（默认 `limited(10)`），
`fetch_text`（:614+）里手写的「3xx 逐跳校验 Location」循环永远不会触发——
`check_public_url` 只校验第一个 URL，公网端点 302 到 `http://127.0.0.1:4763/` 或
`http://169.254.169.254/latest/meta-data` 完整可打。
修复：builder 加 `.redirect(Policy::none())` 激活现有手工校验循环 + 回归测试。

### SEC-P0-2 create_task/edit_task 的 files 参数零校验 → 白名单体系单点破口
`bot.rs:931+` `parse_task_files_arg` 对模型来源的 `{path, isDir}` 只做 trim/去重，
无存在性/来源校验；`bot_fs.rs:114-123` `allowed_dirs()` 把所有任务卡 `is_dir=true`
绑定并入白名单且**不过滤已删除任务**，命中先于 permMode 分流——strict 模式同样失效。
复现：提示注入 → `create_task {files:[{path:"/Users/x", isDir:true}]}` → 主目录进白名单
→ `read_text_file ~/.ssh/config` 静默读取。
修复：模型来源 files 仅放行 AI_Gen_Files 内文件且强制 isDir=false（对齐 link_file_to_task）；
allowed_dirs 过滤已删除任务。

## P1（✅ 已修）

- **SEC-P1-1 grep_files 软链逃逸**（bot_fs.rs walk 回调）：`file_type()` 是 lstat 语义不跟随软链，
  软链文件被当普通文件 grep——`ln -s /etc/hosts ~/Desktop/x.txt` 即可外读。修复：walk 内跳过符号链接。
- **SEC-P1-2 SSRF 地址段遗漏**（bot_web.rs）：IPv4-mapped IPv6（`::ffff:127.0.0.1`）DNS 应答未拦
  （V6 分支只查 loopback/unspecified/ula/link-local）；CGNAT `100.64.0.0/10` 未拦。修复补判。
- **SEC-P1-3 前端直达命令零校验**（bot_skills/files.rs / db.rs）：
  `open_file_path` 任意路径打开（XSS→RCE 一跳）；`delete_bound_file` 任意路径进废纸篓；
  `tasks_export`/`workspace_export` 任意路径写文件。修复：限定任务卡绑定集合 / 数据目录。
- **SEC-P1-4 明文 key 残留**（bot.rs）：Linux 降级明文 key 后 keychain 恢复可用时，
  无任何代码删除 `bot-api-key.txt`（clear 走当前后端，轮不到 PlaintextFile 分支）。
  修复：System 后端可用且降级文件存在时迁回 keychain 并删除。
- **SEC-P1-5 fetch 内存无流式上限**（bot_web.rs）：Content-Length 撒谎时 `bytes()` 全量进内存
  （30s 千兆 ≈ GB 级）。修复：bytes_stream 累计超 2MB 即中断。搜索响应同理补上限。
- **SEC-P1-6 日志注入漏网**：老式 `audit_log`/`py_audit` 不转义，多处用户/模型可控字段直插
  （bot_chat.rs session_id/task_id/meta.name、bot_slash.rs confirm detail、bot_fs.rs 路径、
  scheduler.rs reason 等）——含 `\n` 即可伪造审计行。修复：全部过 escape_for_log。
- **SEC-P1-7 attach_images 无白名单**（bot_chat.rs:181-226）：`[附件文件]` 块图片路径直读外发 LLM，
  不过 permMode 体系。修复：限白名单目录内图片。

## P2（✅ 小项已修；DNS TOCTOU / base_url 校验 / dll 哈希留作记录项）

- key 文件 0600 非原子（先写后 chmod，且 chmod 失败静默）→ OpenOptions mode(0o600) 原子创建
- bot.log 从未设权限（0644，含用户指令/路径）→ 创建时 0600
- `run_dotnet_revisions` 绕过 PY_RUN_GATE 并发闸门 → 补闸门
- `read_text_file` 整读入内存才截断 → 改有界读取
- api_rotate_token 裸 write（0600 依赖文件已存在）→ 复用 write_token_file
- DNS TOCTOU（校验与连接两次解析）：记录为已知残留（缓解需 resolve 钉 IP，改动大）
- LLM base_url 零校验（key 随用户配置外发）：配置面非模型可达，记录建议 https 警告
- .NET dll 无哈希/签名校验（威胁不高于 exe 替换）：记录，发版流水线补
- 审计已核对安全面：LLM key 全链路、SQL 全参数化、params.json 全序列化无拼接、
  API 仅绑 127.0.0.1 + Bearer 全覆盖 + ct_eq + 0600 token、确认机制 fail-closed、
  CSP 无 unsafe-inline、前端无 innerHTML 注入面、opener 禁 file://、无深链
