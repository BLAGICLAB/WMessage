# Batch Spec: MCP-B0

## 目的

按 `docs/AUDIT-FIX-PLAN-2026-09-29.md` B0 批，把工作区未提交的 MCP 外部工具
服务器接入（stdio/HTTP 全链路，阶段 1-7）过完合入前必修后一次合入 main。
审计依据 `AUDIT-FULL-2026-09-29.md` §2；合入同步落地两轮 ocr review 的
4 条 HIGH 与可顺手 MEDIUM（r1 60 条意见 → 修复后 r2 37 条、0 HIGH，余项登记 DEVLOG）。
同批捎带：PKG-1（`py/runtime.rs` impl SetrlimitSupport 漏 `#[cfg(unix)]`，
09-28 Windows 交叉编译 E0425 单行修复，独立 spec 见 PKG-1.spec.md；因
pre-commit worktree_clean 门禁要求无未暂存改动，与 B0 合并落地）。

## 修法（按层）

1. **MCP 功能本体**（bot/mcp/ 五文件 + lib.rs 注册 + dispatch 挂载 + registry 摘尾拼接 +
   McpPanel 设置页 + echo 桩 e2e）：连接槽/懒重连/stderr 环形缓冲；工具命名消歧；
   dispatch miss 反查带超时调用；设置页保存前显式确认弹窗（拍板 3A）。
2. **B0-1 registry 摘尾回退**：`tools_json_with_mcp` base 契约破坏（非 `\n]` 收尾）
   回退静态 base + 限频告警（AtomicUsize，第 1 次 + 每 100 次）；`splice_mcp_body`
   body 侧对偶契约（`,\n` 起头）同样回退；子 agent 判定改显式 subagent_ctx 查表
   （评审：ptr::eq 指针比较对返回形态有隐式契约）。
3. **B0-2 数据面定界**：挂载 schema 8KB 上限降级（宽松空参数+描述标注）、
   structuredContent/总长 30K 字符钳制（mount.rs MAX_SCHEMA_BYTES/MAX_RESULT_CHARS）。
4. **B0-3 e2e 防串扰**：`SHARED_MCP_TEST_LOCK` 串行化触碰进程级单例的测试
   （实施偏差：计划为改局部实例，实际保留生产同路径单例 + 测试锁，理由留 DEVLOG）。
5. **B0-4 面板小项**：timeoutSecs 前端钳 5..=600 整数、800ms 刷新定时器挂卸载清理、
   SaveConfirmDialog http headers 打码、确认弹窗命令行含空白参数加引号（评审 HIGH：
   展示 token 与实际 argv 对齐）。
6. **评审 HIGH/MEDIUM 顺手修**（本轮 ocr）：
   - `execute_mcp_tool` 的 load_config 同步文件读挪 spawn_blocking（不阻塞 tokio worker）；
   - `normalize_server` 空 env/headers 键判定改只按键（`|| 值非空` 会把空键条目
     trim 成空串键留下，保存即被 validate 拒），测试断言同步加强；
   - `spawn_service` 白名单×match 臂双清单：保留字面量构造（Mimosa 闸：无
     「变量→进程名」数据流），新增 parity 单测锁漂移；
   - SSRF 闸加固：纯数字/0x 形式主机名（`http://2130706433/` 类 inet_aton 变体）、
     `.local` mDNS、localhost 家族别名（localhost.localdomain/ip6-localhost/
     broadcasthost）、IPv6 文档段 2001:db8::/32、zone-id 断言；URL 解析失败不回显原文；
   - 非法参数 JSON 不再透传 Null 到远端（本地明确 warn，空串按空对象）；
   - toggle id 不存在响亮报错（DomainRule，不静默）；connected_service 指纹过期
     与未连接分开报；shutdown 批量清 stderr 尾巴；validate_server 补 id 卫生 +
     normalize 补 id trim；拼装 fail-soft 兜底回退静态清单而非空表。

## 红线

- 不动主模型循环既有行为：无 MCP 连接时 tools_json 零分配原样返回（Cow::Borrowed）。
- stdio 启动器白名单语义不变：进程名恒来自常量表字面量（Mimosa 命令注入闸）。
- 子 agent 工具面不变：MCP 工具不进任何 profile 白名单。

## 测试

- cargo：mcp 36（含 e2e echo 桩 roundtrip/非法参数本地拒/parity 锁）、registry 20
  （含摘尾契约双侧回退）、model_loop 44 全绿；vitest SettingsPage 38（含引号展示、
  timeout 钳制回归）；tsc --noEmit 无错；模块地图/桥一致性/错误码三审计过。
- ocr review 两轮：r1 4 HIGH 全修，r2 0 HIGH（13 medium 中 9 顺手修，余登记 DEVLOG）。

## spec 起草后自查三条

1. expected_files 27：pre-commit worktree_clean 门禁强制工作区无未暂存改动 →
   MCP 功能 + B0 修复 + 审计三文档 + DEVLOG（09-28 出包条目 + 本批条目）+ PKG-1
   单批合入（拆分提交被门禁 worktree_clean 检查阻断，见 hook 脚本 check_worktree_clean）。
2. budget：修改 +430/-8（DEVLOG 两段 + 摘尾回退 + 评审修复）；新文件 ~3800 行
   （MCP 五文件 2316 + 面板 820 + 审计文档 373 + specs + 桩）。
3. assertions_min 按 gate 正则（Rust assert 宏、staged 全文计数）填实测-1；
   前端文件不填（gate 只认 Rust 宏，SOP 坑 2）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "MCP-B0",
  "family": "mcp-integration",
  "expected_files": [
    "DEVLOG.md",
    "docs/AUDIT-FIX-PLAN-2026-09-29.md",
    "docs/AUDIT-FULL-2026-09-29.md",
    "docs/MCP-KEYSLOT-MIGRATION-DESIGN-2026-09-29.md",
    "docs/batches/MCP-B0.spec.md",
    "docs/batches/PKG-1.spec.md",
    "docs/rust-bot-architecture.md",
    "src-tauri/Cargo.lock",
    "src-tauri/Cargo.toml",
    "src-tauri/src/bot.rs",
    "src-tauri/src/bot/config/commands.rs",
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot/dispatch.rs",
    "src-tauri/src/bot/mcp/commands.rs",
    "src-tauri/src/bot/mcp/config.rs",
    "src-tauri/src/bot/mcp/manager.rs",
    "src-tauri/src/bot/mcp/mod.rs",
    "src-tauri/src/bot/mcp/mount.rs",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/lib.rs",
    "src-tauri/src/py/runtime.rs",
    "src-tauri/tests/fixtures/mcp_echo_server.py",
    "src/components/SettingsPage/McpPanel.test.tsx",
    "src/components/SettingsPage/McpPanel.tsx",
    "src/components/SettingsPage/SettingsPage.tsx",
    "src/components/SettingsPage/types.ts"
  ],
  "max_lines_added": 700,
  "max_lines_removed": 20,
  "max_new_files_lines": 4200,
  "findings": [
    {"id": "B0-1", "file": "src-tauri/src/bot/registry.rs", "line": 949, "fix": "摘尾拼接双侧契约回退 + 限频告警 + 子 agent 判定显式化"},
    {"id": "B0-2", "file": "src-tauri/src/bot/mcp/mount.rs", "line": 40, "fix": "数据面定界：schema 8KB 降级 / 结果 30K 钳制 / structuredContent 先钳后入栈"},
    {"id": "B0-H1", "file": "src-tauri/src/bot/mcp/config.rs", "line": 240, "fix": "评审 HIGH：normalize 空键只按键判定丢弃；SSRF 补数字形式/.local/localhost 别名/db8 段；id 卫生"},
    {"id": "B0-H2", "file": "src-tauri/src/bot/mcp/manager.rs", "line": 280, "fix": "评审 HIGH：白名单×match 双清单 parity 单测锁；connected_service 拆指纹过期错；shutdown 清 stderr"},
    {"id": "B0-H3", "file": "src-tauri/src/bot/mcp/mount.rs", "line": 160, "fix": "评审 HIGH：execute_mcp_tool 的 load_config 挪 spawn_blocking；非法参数 JSON 本地拒"},
    {"id": "B0-H4", "file": "src/components/SettingsPage/McpPanel.tsx", "line": 100, "fix": "评审 HIGH：确认弹窗含空白参数加引号；+timeout 钳制/单定时器防抖/展开请求序号/startEdit 重入闸"},
    {"id": "PKG-1", "file": "src-tauri/src/py/runtime.rs", "line": 457, "fix": "impl SetrlimitSupport 补 #[cfg(unix)]（Windows-gnu E0425）"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/registry.rs": 40,
    "src-tauri/src/bot/mcp/config.rs": 100,
    "src-tauri/src/bot/mcp/manager.rs": 24,
    "src-tauri/src/bot/mcp/mount.rs": 25
  },
  "ocr_plan": {
    "rounds": 2,
    "timeout_seconds": 900,
    "expected_max_comments": 40
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```
cargo test mcp && cargo test registry && cargo test model_loop
npx vitest run src/components/SettingsPage && npx tsc --noEmit
python3 -m pytest tests-audit/ -q
```
