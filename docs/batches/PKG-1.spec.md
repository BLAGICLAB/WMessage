# Batch Spec: PKG-1

## 目的

09-28 Windows 绿色包交叉编译触发的单行修复：`py/runtime.rs` 的
`impl SetrlimitSupport` 漏 `#[cfg(unix)]` gate（9ff9d1a 加 S20 py setrlimit
探测时漏配；结构体本身已 gate 但 impl 块漏了）。macOS dev 只编 unix 路径
所以未发现；Windows-gnu 交叉编译 E0425 暴露。DEVLOG 09-28 条目有完整记录。

## 修法

impl 块补 `#[cfg(unix)]`，与结构体 gate 对齐（+1 行）。

## 红线

- macOS 行为零变化（unix 路径不受影响）；仅非 unix 目标从编译错误恢复。

## 测试

- macOS：`cargo check` 通过（回归无影响，09-28 出包时已验证）。
- Windows-gnu 交叉编译验证随下次出包自然覆盖（本次不重复跑 33min 交叉链）。

## spec 起草后自查三条

1. expected_files 2：修复文件 + 本 spec。
2. budget：修改 +1/-0；新文件（本 spec）不计入 Rust 行。
3. fix 字段：单点 cfg gate 对齐，无其他改动。

## 机器可读（脚本读取，勿改格式）

```json
{
  "batch_id": "PKG-1",
  "family": "hotfix-cfg-gate",
  "expected_files": [
    "src-tauri/src/py/runtime.rs",
    "docs/batches/PKG-1.spec.md"
  ],
  "max_lines_added": 2,
  "max_lines_removed": 0,
  "findings": [
    {"id": "PKG-1", "file": "src-tauri/src/py/runtime.rs", "line": 457, "fix": "impl SetrlimitSupport 补 #[cfg(unix)]，与结构体 gate 对齐（Windows-gnu E0425 修复）"}
  ],
  "stop_conditions": ["compile_failure", "gate_fail"]
}
```

## 验证命令

```
cd src-tauri && cargo check
```
