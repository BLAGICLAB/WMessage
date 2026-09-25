# Batch Spec: APW-02a

## 目的

C5-MI-06：`copy_dir_recursive`（ops.rs:127）当前**dst 直接 create_dir_all + 递归 fs::copy 直写 dst**——迭代失败中途，dst 留半截内容，无回滚。

修法：顶层建 staging sibling（`dst.parent().join(format!("{}.copying", file_name))`，显式追加不用 `with_extension`），递归直写 staging（无子级 staging），顶层一次 `fs::rename(staging, dst)` 原子 commit。失败 `fs::remove_dir_all(&staging)` 清 staging，dst 未被触碰。

新增 top-level **symlink + 已存在** 防护：
- src 是 symlink → 返 Err（`fs::read_dir(symlink)` 跟读目标，entries 与用户预期不一致，move 语义失控）
- dst 已存在 → 返 Err（保守语义，不改现有"dst 不存在"行为；防御性 invariant）

helper `copy_dir_recursive_into` 内部继续保留 entry-level symlink 检查（防御子目录内 symlink，与 top-level 检查对称）。

family = **atomicity-partial-write**（成员 +1）。本批与 APW-02b（copy_legacy_db）拆开，理由：scope 不同（FS 递归 vs 3 文件 swap）+ 预算 136 行巨批单批 OCR 面过大 + 回滚逻辑不同（清 staging vs 文件删除）。

## 人类可读摘要

- family: atomicity-partial-write
- 覆盖 findings: 1（C5-MI-06）
- 预估 diff: 1 file / +61/-21 lines
- OCR 计划: r1, timeout 1800s, 期望 comments ≤ 5

## 红线

- family 一致性：本批只含 atomicity-partial-write 一条 finding（拆 APW-02a，APW-02b 另批）
- 不写"待定" / "同结构改"（按 SPEC-001 教训）
- 不引入 silent fallback / log-and-continue 形态
- 公开 API `copy_dir_recursive(src, dst) -> Result<(), CommandError>` 不变（新增 private helper）
- 跨卷行为：staging 在 dst 同 volume，rename atomic
- 对 dst 已存在：返 Err（保守语义，不改现有"dst 不存在"行为）
- 对 src 是 symlink：返 Err（top-level 检，read_dir 跟读目标 = move 语义失控）
- 对 entry-level symlink：保留（防御子目录内 symlink，与 top-level 对称）

## Stop 条件

- compile_failure / architecture_blocker / family_heterogeneity / new_high_different_root / gate_fail

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "APW-02a",
  "family": "atomicity-partial-write",
  "expected_files": [
    "src-tauri/src/migration/ops.rs",
    "src-tauri/src/db/mod.rs"
  ],
  "max_lines_added": 86,
  "max_lines_removed": 3,
  "findings": [
    {
      "id": "C5-MI-06",
      "file": "src-tauri/src/migration/ops.rs",
      "line": 127,
      "fix": "公开 API 签名不变：`pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), CommandError>`。函数体改为：(1) top-level src symlink 检查 — `if src.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false)` → 返 Err(CommandError::DomainRule{domain: \"migration\", reason: format!(\"src 是符号链接，read_dir 跟读目标，entries 与用户预期不一致：{}\", src.display())})；(2) top-level dst 已存在检查 — `if dst.symlink_metadata().is_ok()` → 返 Err(DomainRule)；(3) 顶层 staging sibling 构造 — `let staging = dst.parent().ok_or_else(...)?.join(format!(\"{}.copying\", dst.file_name().ok_or_else(...)?.to_string_lossy()))`（不用 with_extension）；(4) 递归调 helper `copy_dir_recursive_into(src, &staging)`，失败 `fs::remove_dir_all(&staging)` + return Err；(5) `fs::rename(&staging, dst)` 原子 commit。新增 private helper `fn copy_dir_recursive_into(src: &Path, dst: &Path) -> Result<(), CommandError>` —— 搬旧 `copy_dir_recursive` body，`fs::create_dir_all(dst)` + `fs::read_dir(src)` 迭代 entries（entry-level symlink 检查保留），子目录递归 helper，文件 fs::copy。helper 不建子级 staging，完整镜像 src 到 staging。【同 family 与 APW-01 不同点】函数层 fail-closed（rename 失败 clear staging + Err）；非 DB transaction 包裹（filesystem 层 atomicity）。"
    }
  ],
  "ocr_plan": {
    "rounds": 1,
    "timeout_seconds": 1800,
    "expected_max_comments": 5
  },
  "stop_conditions": [
    "compile_failure",
    "architecture_blocker",
    "family_heterogeneity",
    "new_high_different_root",
    "gate_fail"
  ]
}
```

## 签名核（执行前已验）

- 公开 `copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), CommandError>` —— 不变
- 新增 private `copy_dir_recursive_into(src: &Path, dst: &Path) -> Result<(), CommandError>` —— 同签名模式
- 旧 body 22 行（ops.rs:127-149）整体搬到 helper（`dst.join(entry.file_name())` 路径不变）

## 调用点影响清单

| # | caller | 行号 | 形态 | 影响 |
|---|---|---|---|---|
| 1 | ops.rs:154 move_entry 跨卷路径 | `copy_dir_recursive(src, dst)` | 返 Err 形态变化（原"覆盖语义"→ 新"拒绝 dst 已存在 + 拒绝 src symlink"） | caller 行为不变（新检查在现有调用场景下不触发） |
| 2 | helper `copy_dir_recursive_into` 内部递归 | `copy_dir_recursive_into(&s, &d)` | 同签名内部调用 | 不变 |
| 3 | mod.rs:547-554 测试 `copy_dir_recursive_works` | 调 `copy_dir_recursive(&src, &dst).unwrap()` | 函数名/签名不变 | 不动 |
| 4 | **新增测试 `copy_dir_recursive_rejects_existing_dst`** | tempdir 内预建 dst → 调 → 断言 `Err` 且 dst 原样未变 | 单测可稳定构造 |
| 5 | **新增测试 `copy_dir_recursive_rejects_symlink_src`** | tempdir 内 `std::os::unix::fs::symlink(real_src, sym_src)` 建 symlink → 调 → 断言 `Err` | 单测可稳定构造 |

## Budget 逐点算（SOP 第 2 条）

| # | 改动点 | + | - | 说明 |
|---|---|---|---|---|
| 1 | 顶层 src symlink top-level 检查 | 6 | 0 | read_dir 跟读目标 = move 语义失控 |
| 2 | 顶层 dst 已存在检查 | 6 | 0 | 保守语义 |
| 3 | 顶层 parent 提取 | 4 | 0 | |
| 4 | 顶层 file_name 提取 | 4 | 0 | |
| 5 | 顶层 staging 构造 | 1 | 0 | 显式追加，不用 with_extension |
| 6 | 顶层调用 + Err 分支 | 4 | 0 | |
| 7 | 顶层 rename 提交 | 1 | 0 | |
| 8 | 顶层 Ok(()) | 1 | 0 | |
| 9 | helper fn 头 | 1 | 0 | |
| 10 | helper body（搬旧 body，递归调用名改） | 21 | 21 | |
| 11 | 顶层旧 body 删除 | 0 | 21 | |
| 12 | 测试 `copy_dir_recursive_works`（mod.rs:547-554） | 0 | 0 | 不动 |
| 13 | **新增测试 `copy_dir_recursive_rejects_existing_dst`** | 6 | 0 | tempdir 预建 dst → 调 → 断言 Err + dst 未变 |
| 14 | **新增测试 `copy_dir_recursive_rejects_symlink_src`** | 6 | 0 | tempdir 建 symlink → 调 → 断言 Err |

合计：+61 / -21
budget max_lines_added: 65 / max_lines_removed: 25（± 4 行 buffer）

## 提交信息骨架

```
fix(migration): APW-02a — copy_dir_recursive 顶层 staging + 原子 rename（atomicity-partial-write family +1）

【family】atomicity-partial-write（成员 +1：C5-MI-06）
C5-MI-06：copy_dir_recursive 当前 dst 直接 create_dir_all + 迭代 fs::copy 直写 dst。
迭代失败中途 dst 留半截内容，无回滚。

【实修】（1 文件）
- migration/ops.rs:127（finding 原发）：
  ① top-level src symlink 检查（read_dir 跟读目标 = move 语义失控）→ 返 Err
  ② top-level dst 已存在检查（保守语义，不改现有"dst 不存在"行为）→ 返 Err
  ③ 顶层 staging sibling 构造：dst.parent().join(format!("{}.copying", dst.file_name()))
     （不用 with_extension，避免 .tar/.pdf 等扩展名被替换错）
  ④ 递归调 helper `copy_dir_recursive_into(src, &staging)`，失败
     `fs::remove_dir_all(&staging)` + return Err
  ⑤ `fs::rename(&staging, dst)` 原子 commit（同 parent 同 volume）
- 新增 private helper `copy_dir_recursive_into(src, dst)`：搬旧 body（fs::create_dir_all +
  read_dir 迭代 entries，entry-level symlink 检查保留，子目录递归 helper，文件 fs::copy）。
  helper 直写 staging，不建子级 staging。

【自测】cargo fmt / cargo check --tests / scripts/test-all.sh 全绿
- 测试 copy_dir_recursive_works（mod.rs:547-554）函数名/签名不变，不动
- 新增 copy_dir_recursive_rejects_existing_dst（tempdir 预建 dst → 调 → 断言 Err）
- 新增 copy_dir_recursive_rejects_symlink_src（tempdir 建 symlink → 调 → 断言 Err）

【OCR】r1：期望 ≤ 5 comments（1 文件改动 + 2 个新增 top-level 检查 + 函数搬移 + 2 个新测试）

【family】atomicity-partial-write（成员 +1：C5-MI-06）
【拆出】C5-DB-02a.paths copy_legacy_db 拆 APW-02b（v4 caller 草案待出 spec 时一并发你审）
```

## 验证命令

```bash
BATCH_SPEC=docs/batches/APW-02a.spec.md python3 scripts/batch-verify.py docs/batches/APW-02a.spec.md
```

## 不在本批

- C5-DB-02a.paths copy_legacy_db → APW-02b
  - 拆批理由：scope 不同（FS 递归 copy vs 3 文件 swap）+ 预算 136 行巨批（单批 OCR 面过大）+ 回滚逻辑不同
  - 拆批时间线：APW-02a 先跑；APW-02b 等你拍 v4 caller 草案后出 spec