# Batch Spec: EV-B4

## 目的

按 `docs/AUDIT-FIX-PLAN-2026-09-29.md` B4 批（P2 组 + 拍板②⑤；B4-6 KeySlot
迁移单独成批）：

- **B4-1（P2-EV8）eval_run 路径修正**：applied 路径优先级改为
  `--applied` 覆盖 > db 同目录（applied.jsonl 与 DB 同在 data_dir）> eval_set
  同目录兜底——此前直接用兜底会读不存在的 `<eval_set>.applied.jsonl`（空
  applied → 回滚率/污染存活期恒 0）。`case_passed` 100% 占位在输出显式标注
  （`MetricsReport.case_passed_placeholder`，serde default 兼容旧结果）。
- **B4-2（P2-EV10）trace 接真实数据**：`run_model_loop` 返回追加
  `LoopTrace`（轮数 = llm.request 审计计数；工具明细 = 薄壳 execute_tool
  包装器计时+统一成败口径采集，真实分发路径零改动）；bot_chat 主路径
  Failure 分支可达（模型循环 Err 也落 trace，粗分类 reason 不带原始消息）；
  trace.rs 陈旧「Phase 1 占位」注释刷新。四个调用方同步适配。
- **B4-3（P2-EV11）jsonl 损坏自愈**：`read_jsonl` 任何行损坏 → 先整体备份
  `.corrupt-<ts>` 再跳坏行（首行损坏不再永久 fail-closed——一个坏字节曾让
  面板永久打不开且无自愈出口）；备份是取证/手工修复入口。两个 fail-closed
  测试改写为自愈断言（备份存在 + 好行返回）。
- **B4-4（P3 组）panel 纪律**：cascade 删除与 rollback 删 mem_item 两处
  裸连接写持 `DB_WRITE_LOCK`（锁序安全：apply 先释放 DB 锁再取 EVO 锁，
  panel 反序，无环）；toggle ON 终态校验（Rejected/Expired 不可复活，在
  EVOLUTION_STORE_LOCK 内 = promote 段 B 的段间复检；RolledBack 复用走
  前端二次确认放行）。
- **B4-5（拍板②）kill_switch 真接线**：`apply_from_consolidation` 入口现读
  bot-config.json 的 `evolution.kill_switch`（方法文档「apply.rs 入口检查」
  自此为真）；shadow_only/all_auto_apply → 跳过主 apply（shadow 钩子照常，
  kill 只停写不强制开观察）；disable_notification → 完成摘要行静默。
  实验态模块头标注：candidate/ttl、candidate/conflict、sandbox/routing、
  sandbox/mod（routing/shadow/io）、activation 的 save_state/shadow_route
  （零调用，接口按 spec 冻结等真数据）。
- **拍板⑤**：run_python schema 文案「本机沙箱」→「资源受限：CPU/内存/时长
  限额 + 独立临时目录，无文件系统隔离」。

## 红线

- kill_switch 配置缺块 = 全关默认（行为与现状一致）；kill 只停「写」不强制
  开「观察」（shadow 仍受 evolution.shadow.enabled 独立控制）。
- trace 的 reason 用粗分类（model_loop_error），不带原始错误消息（路径/参数
  不入 trace/audit 明细）。
- toggle 的 RolledBack 复用语义不变（前端二次确认，拍板①）。

## 测试

- 改写 2 个（fail-closed → 自愈：备份存在 + 好行返回）；eval 占位字段断言
  （runner 测试字面量补字段）。
- 回归：evolution 283 / eval 38 / task_chat_exec 14 / llm_integration 37 /
  memory 56 / model_loop 44 / py 64 全绿；cargo check --all-targets 0 error。

## spec 起草后自查三条

1. expected_files 21：十九个源文件 + DEVLOG + 本 spec。
2. budget：修改 +430/-190（apply.rs 的 if/else 重排 + mod.rs read_jsonl 重写
   占大头）。
3. assertions_min 按 gate 正则填实测-1（staged 全文计数）。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "EV-B4",
  "family": "evolution-tooling",
  "expected_files": [
    "DEVLOG.md",
    "docs/batches/EV-B4.spec.md",
    "src-tauri/src/bot/registry.rs",
    "src-tauri/src/bot_chat.rs",
    "src-tauri/src/bot_model_loop.rs",
    "src-tauri/src/bot_orchestrator.rs",
    "src-tauri/src/bin/eval_run.rs",
    "src-tauri/src/eval/metrics.rs",
    "src-tauri/src/eval/runner.rs",
    "src-tauri/src/exec_steps.rs",
    "src-tauri/src/evolution/activation.rs",
    "src-tauri/src/evolution/apply.rs",
    "src-tauri/src/evolution/candidate/conflict.rs",
    "src-tauri/src/evolution/candidate/entry.rs",
    "src-tauri/src/evolution/candidate/ttl.rs",
    "src-tauri/src/evolution/change/record.rs",
    "src-tauri/src/evolution/mod.rs",
    "src-tauri/src/evolution/panel/commands.rs",
    "src-tauri/src/evolution/sandbox/mod.rs",
    "src-tauri/src/evolution/sandbox/routing.rs",
    "src-tauri/src/evolution/trace.rs"
  ],
  "max_lines_added": 560,
  "max_lines_removed": 250,
  "findings": [
    {"id": "B4-1", "file": "src-tauri/src/eval/runner.rs", "line": 26, "fix": "applied 路径优先级（--applied > db 同目录 > 兜底）+ case_passed 占位标注"},
    {"id": "B4-2", "file": "src-tauri/src/bot_model_loop.rs", "line": 429, "fix": "LoopTrace 回传（轮数=llm.request 计数 / 工具明细=薄壳包装器采集）"},
    {"id": "B4-2", "file": "src-tauri/src/bot_chat.rs", "line": 933, "fix": "Failure 分支落 trace + turn/tool_calls 接真实数据"},
    {"id": "B4-3", "file": "src-tauri/src/evolution/mod.rs", "line": 28, "fix": "read_jsonl 自愈：.corrupt-<ts> 备份 + 跳坏行（首行不再 fail-closed）"},
    {"id": "B4-4", "file": "src-tauri/src/evolution/panel/commands.rs", "line": 101, "fix": "toggle 终态校验（锁内=段间复检）+ 两处 mem 写持 DB_WRITE_LOCK"},
    {"id": "B4-5", "file": "src-tauri/src/evolution/apply.rs", "line": 158, "fix": "kill_switch 真接线（现读现判：shadow_only 跳主 apply / 通知静默）"},
    {"id": "拍板⑤", "file": "src-tauri/src/bot/registry.rs", "line": 222, "fix": "run_python schema 文案：本机沙箱 → 资源受限，无文件系统隔离"}
  ],
  "assertions_min": {
    "src-tauri/src/evolution/apply.rs": 38,
    "src-tauri/src/evolution/panel/commands.rs": 25,
    "src-tauri/src/eval/runner.rs": 9,
    "src-tauri/src/bot_model_loop.rs": 122,
    "src-tauri/src/bot_chat.rs": 68
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 900,
    "expected_max_comments": 45
  },
  "stop_conditions": ["compile_failure", "architecture_blocker", "family_heterogeneity", "new_high_different_root", "gate_fail"]
}
```

## 验证命令

```
cargo test evolution:: && cargo test eval && cargo test --test task_chat_exec && cargo test --test llm_integration
cargo test memory && cargo test model_loop && cargo test py
```
