# BT-08a spec — types.rs:82 序列化纵深 + parse.rs:86 非法 mode 修正（C5-BT-08 拆批 1/2）

## 拆批与处置（5 条全登记）

1. **types.rs:198**（默认模型 deepseek-v4-flash）→ **FP（web 实证）**：deepseek.ai/pricing
   （2026-09-18）——V4.1-Flash 2026-09-09 GA 后服务名 `deepseek-flash`，旧名
   `deepseek-v4-flash` **仍被接受**（请求由 V4.1-Flash 服务）；finding 前提
   「无此模型 / 新装首调 404」不实。是否跟进改名 `deepseek-flash` = 产品决策，
   不阻塞本批。来源：https://deepseek.ai/pricing + tech-insider.org V4.1 教程。
2. **types.rs:82**（本批）：api_key/tavily_key/brave_key 三字段
   `skip_serializing_if = "Option::is_none"` → **`skip_serializing`**（永不序列化）。
   核实：前端经 BotConfigView 拿 has_* 布尔不拿 key 材料（commands.rs:73-91）；
   三条写路径本就剥 None 写盘（io.rs write_bot_config_file_locked）；迁移靠
   **反序列化**读老文件（skip_serializing 不影响 deserialize）；mod.rs:220 的
   "***" 是测试输入串非掩码机制。当前路径零行为变更；任何未来忘剥 key 的写
   路径 = fail-closed（key 不落盘）。
3. **keyring.rs:168**（Windows 明文回退无 ACL）→ **B 类攒批第 13 项**：修法 =
   winapi DACL，macOS 上不可测试；且 Windows 明文回退已是 keyring 全失败后的
   最后手段。方向（加 ACL vs 回退路径整体拒写 vs 文档化接受）报人拍。
4. **parse.rs:86**（本批）：`mode_explicit = true` 在合法性校验**之前**置位——
   `mode: bg` + `risk_level: low` 时非法值吞掉 low→auto 提升（未声明反而提升）。
   修：合法值才置 mode_explicit；非法值按未声明处理（mode 落默认/提升逻辑接管）。
5. **vars.rs:76**（in_quotes 字节启发式）→ **wontfix-with-rationale（待醒后确认）**：
   替换前的 args 模板含 `${...}` 占位符，**构造上不是合法 JSON**，完整 JSON
   解析器无法解析它——启发式是结构性必然不是偷懒。风险面 = 模型生成的 args
   里引号形态诡异时转义误判，但替换后还会过 serde_json 解析（非法 JSON 在
   工具层报错不执行）。技术上 wontfix，非设计意图型，醒后确认。

## 目标 findings（2 条实修）

- types.rs:82：三字段 serde attr 改 + 注释同步（3 行）。
- parse.rs:86：mode_explicit 移到合法分支内 + 注释；新增测试
  `meta_invalid_mode_does_not_block_low_auto_promotion`（mode: bg + low → auto；
  mode: interactive + low → interactive 保留；mode: auto + low → auto 保留）。

## 行为变更

- types.rs：零（当前写路径本就剥 None；Some 序列化路径不存在）。
- parse.rs：`mode: 非法值` + `risk_level: low` 的 Skill 从「滞留 interactive」变
  「按未声明提升 auto」——这是 finding 指正的语义，SKILL_DSL.md「low 强制 auto」
  约定归一。合法 mode 行为不变。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/bot/config/types.rs` +
   `src-tauri/src/bot_skills/parse.rs`。无签名 ripple。✓
2. budget → types 3 行 attr + 注释 ≈+4/-3；parse 改动 + 测试 ≈+16/-4。
   合计 ≈+20/-7 → budget +28/-10。✓
3. findings fix 字段列 ripple → 均无。✓
4. （执行中校正）零代码处置三条（:198 FP / :168 B 类 / :76 wontfix）从机器可读
   findings 数组移除——gate findings_files_in_diff 要求每条 finding 的 file 在
   staged 集合内，零代码条目的文件不可能 staged。处置记录以本文「拆批与处置」
   节为准，随 triage 收口 docs commit 登记。
5. （执行中校正 2）expected_files 加 `src-tauri/src/bot/config/mod.rs`——OCR r1
   低危指出 mod.rs:541 测试注释仍写旧机制名（skip_serializing_if），注释同步
   1 行；numstat 合计 +21/-6 仍在 budget 内。

## 自主执行规则

spec 被 reviewer 批准后:
- agent 全权执行至 commit,不中途报 status / exit / diff
- 例外只有 stop 条件触发
- 完成时一次报: hash + 批定性 + OCR 轮次 + comments 处置 + follow-up
- 过程证据落 ~/.openclaw/cache/<batch-id>/,不主动读回

## Stop 条件(触发即停,报 reviewer)

- compile_failure (无 X 形态退路)
- architecture_blocker (签名/调用点超预算)
- family_heterogeneity
- new_high_different_root (OCR r1 出的 high 非本批根因)
- gate_fail (batch-verify FAIL 且非 spec 声明调整可解)

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "BT-08a",
  "family": "serialization-defense-and-parse-validation",
  "expected_files": [
    "src-tauri/src/bot/config/types.rs",
    "src-tauri/src/bot_skills/parse.rs",
    "src-tauri/src/bot/config/mod.rs"
  ],
  "max_lines_added": 28,
  "max_lines_removed": 10,
  "findings": [
    {"id": "C5-BT-08.2", "file": "src-tauri/src/bot/config/types.rs", "line": 82, "fix": "api_key/tavily_key/brave_key 三字段 skip_serializing_if → skip_serializing（永不序列化明文 key，fail-closed 纵深）。ripple：无（写路径本就剥 None，输出等价）"},
    {"id": "C5-BT-08.4", "file": "src-tauri/src/bot_skills/parse.rs", "line": 86, "fix": "mode_explicit 移到合法值分支内置位；非法 mode 按未声明处理（low→auto 提升不被吞）。新增 3 断言测试。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/bot/config/types.rs": 0,
    "src-tauri/src/bot_skills/parse.rs": 81
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  }
}
```
