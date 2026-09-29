# Batch Spec: FIXT-1

## 目的

test-all 全量闸（§10 验收）拦截的基线漂移：EV-B4 拍板⑤改 run_python schema
文案（「本机沙箱」→「资源受限…无文件系统隔离」）时漏同步
`tests/fixtures/tools_baseline.json`，`tools_json_matches_baseline` 失败——
这正是基线锁的设计用途（schema 漂移必被逮住）。同步 fixture 即修复。

## 修法

fixture 的 run_python description 与 registry.rs SCHEMA_RUN_PYTHON 对齐
（+2/-2 行）。无其他变更。

## 红线

- 基线锁语义不变：前 29 项逐字节比对、总数 29+3。

## 测试

- `cargo test tools_json_matches_baseline` 过；`bash scripts/test-all.sh`
  exit 0（1277 tests 全绿，本批提交前已实测）。

## spec 起草后自查三条

1. expected_files 2：fixture + 本 spec。
2. budget：修改 +2/-2。
3. fix 字段单点。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "FIXT-1",
  "family": "hotfix-fixture",
  "expected_files": [
    "docs/batches/FIXT-1.spec.md",
    "src-tauri/tests/fixtures/tools_baseline.json"
  ],
  "max_lines_added": 4,
  "max_lines_removed": 2,
  "findings": [
    {"id": "FIXT-1", "file": "src-tauri/tests/fixtures/tools_baseline.json", "line": 120, "fix": "run_python description 与拍板⑤文案对齐（资源受限/无文件系统隔离）"}
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
cargo test tools_json_matches_baseline && bash scripts/test-all.sh
```
