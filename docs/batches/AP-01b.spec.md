# AP-01b spec — TOCTOU 原子化：token 首建 + worker 上限（C5-AP-01 拆批 b）

## 拆批说明

C5-AP-01 四条三设施：commands.rs:61/:238（「文件 I/O 不持锁」模块约定 vs
flag 原子性）= **改既有约定方向 → B 类攒批第 15 项**（A=flag 写清移入锁内 /
B=写后校验收敛 / C=wontfix 窗口小后果轻）。本批只收另两条无争议的
「原子化 check-then-act」（同 family 同主题，两文件各一设施）：

## 目标 findings（2 条实修）

- **api_auth.rs:27**：`load_or_create_token` check-then-act——并发首跑（双开
  应用）两进程各生成 UUID，后写覆盖先写，输家返回与磁盘不符的 token。
  修：新增 `create_token_file_atomic`（OpenOptions create_new + unix 0600，
  写最终路径非 tmp）——原子占位唯一胜者；AlreadyExists → 重读胜者 token
  （create 与 write_all 间空窗：短暂重试 ≤500ms，超时仍空报错冒泡下次自愈）。
  覆写/轮换仍走 write_token_file（tmp+rename）不动。新增测试
  `create_token_file_atomic_first_wins`（首建 Ok(true)+内容+0600；二次
  Ok(false) 不覆盖）。
- **api_server.rs:208**：worker 上限 load+fetch_add 两步竞态（burst 下并发
  超 MAX_WORKERS）。修：`fetch_add` 返回值即占位序号，超限 fetch_sub 归还
  + 503（finding 建议原案）。

## 行为变更

- api_auth：并发首跑从「输家 token 与磁盘不符」变「输家读胜者 token」；
  单进程路径零变化。空窗重试最坏 +500ms 延迟（仅并发首跑输家）。
- api_server：worker 计数严格 ≤ MAX_WORKERS（原 burst 可瞬时超限）。

## spec 起草后自查三条

1. `expected_files` → `src-tauri/src/api_auth.rs` + `src-tauri/src/api_server.rs`。
   无签名 ripple（load_or_create_token 签名不变）。✓
2. budget → api_auth helper+调用改+测试 ≈+32/-2；api_server ≈+7/-5。
   合计 ≈+39/-7 → budget +48/-12。✓
   （执行中校正）实际 +62/-5：create_new helper 注释 + 空窗重试循环 +
   0600 断言测试比估算长；budget 校正 +48→+68。✓
3. findings fix 字段列 ripple → 均无。✓

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
  "batch_id": "AP-01b",
  "family": "atomic-check-and-act",
  "expected_files": [
    "src-tauri/src/api_auth.rs",
    "src-tauri/src/api_server.rs"
  ],
  "max_lines_added": 68,
  "max_lines_removed": 12,
  "findings": [
    {"id": "C5-AP-01.1", "file": "src-tauri/src/api_auth.rs", "line": 27, "fix": "create_token_file_atomic（create_new+0600 原子占位）；输家重读胜者 token（≤500ms 空窗重试，超时报错自愈）。新增 3 断言测试。ripple：无（签名不变）"},
    {"id": "C5-AP-01.4", "file": "src-tauri/src/api_server.rs", "line": 208, "fix": "fetch_add 返回值原子占位，超限 fetch_sub 归还 + 503。ripple：无"}
  ],
  "assertions_min": {
    "src-tauri/src/api_auth.rs": 14,
    "src-tauri/src/api_server.rs": 11
  },
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 4
  }
}
```
