# C 类补全建议 — Comment Hygiene 2026-09-15

> **状态**：仅建议，**未执行**。用户回来拍板。
> **原则**：SAFETY 内容**直接写在 `// SAFETY:` 注释里**，不引用 pyO3 / Windows / Apple 文档等外部链接。
> 一文件一个 patch，方便回滚。

## C-1 — `unsafe` 块全部缺 SAFETY 说明（**高优先级**）

`src-tauri/src/` 共 **9 处** `unsafe` 块，**全部没有 SAFETY 注释**。
分布：macOS Foundation FFI ×3、Win32 Job Object FFI ×4、POSIX libc ×2。

> **修正前版（已作废）**：原 C-additions.md 把 Win32 / libc unsafe 误标为 pyO3，下面是勘误后的实际归类。

### C-1-1 ~ 1-3 — macOS NSPasteboard FFI（`platform/copy_file.rs:40/44/65`）

**C-1-1** `platform/copy_file.rs:40`
```rust
unsafe { pb.setPropertyList_forType(&paths, &NSString::from_str("NSFilenamesPboardType")) };
```
**建议 SAFETY**：
```
// SAFETY: &NSString 由 NSString::from_str 创建，存于本栈帧，调用期间不释放；
// &paths 是 CFArray 借用视图（paths 已 validate 非空），调用方保证生命周期。
// Foundation `setPropertyList:forType:` 在 valid NSPasteboard + valid NSString/NSArray 引用上是安全的。
```

**C-1-2** `platform/copy_file.rs:44`
```rust
let _ok = pb.setString_forType(&text, unsafe { NSPasteboardTypeString });
```
**建议 SAFETY**：
```
// SAFETY: NSPasteboardTypeString 是 Foundation 公开常量（`'public static NSString * const`），
// 值稳定不释放、全局唯一无别名风险；unsafe 仅用于将 `*const NSString` 转为 `&NSString`。
```

**C-1-3** `platform/copy_file.rs:65`（unsafe 块入口）
```rust
unsafe {
    ... // 含 1-1 + 1-2 的实际调用
}
```
**建议 SAFETY**：
```
// SAFETY: 块内调用见各行 SAFETY；本 unsafe 块仅作代码组织
//（块内需要 unsafe 关键字才能调底层 FFI），并非单行 unsafe。
```

---

### C-1-4 ~ 1-7 — Win32 Job Object FFI（`py/runtime.rs:67/92/98/105`）

**C-1-4** `py/runtime.rs:67`（`create`：建 Job + 设限制）
```rust
unsafe {
    let job = CreateJobObjectW(None, PCWSTR::null()).ok()?;
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = ...;
    let r = SetInformationJobObject(job, JobObjectExtendedLimitInformation,
        &info as *const _ as *const std::ffi::c_void,
        std::mem::size_of_val(&info) as u32);
    if r.is_err() { let _ = CloseHandle(job); return None; }
    Some(JobGuard(job))
}
```
**建议 SAFETY**：
```
// SAFETY: CreateJobObjectW(NULL, NULL) = 默认安全描述符、不命名（防跨进程名冲突）；
// &info 是有效的 Rust 结构体，size_of_val 在编译期校验大小匹配 Win32 期望。
// SetInformationJobObject 失败时已 CloseHandle 兜底，不会泄漏 HANDLE。
```

**C-1-5** `py/runtime.rs:92`（`assign`：把子进程绑到 Job）
```rust
unsafe {
    let _ = AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle() as _));
}
```
**建议 SAFETY**：
```
// SAFETY: child.as_raw_handle() 由 std 保证是 valid HANDLE（子进程已 spawn）；
// job.0 由本模块 CreateJobObjectW 创建并由 JobGuard 持有；
// 返回值忽略：AssignProcessToJobObject 失败时子进程仍可由 TerminateJobObject 强杀。
```

**C-1-6** `py/runtime.rs:98`（`terminate`：强杀整个 Job）
```rust
unsafe {
    let _ = TerminateJobObject(job.0, 1);
}
```
**建议 SAFETY**：
```
// SAFETY: job.0 是 valid HANDLE（CreateJobObjectW 已返回 Ok）；
// TerminateJobObject 对 valid HANDLE 调用安全，退出码 1 仅触发清理，无语义依赖。
```

**C-1-7** `py/runtime.rs:105`（Drop for JobGuard：释放 HANDLE）
```rust
impl Drop for JobGuard {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); }
    }
}
```
**建议 SAFETY**：
```
// SAFETY: self.0 由 CreateJobObjectW 创建并独占持有；
// Drop 必须 CloseHandle 以释放内核对象，否则 HANDLE 泄漏到进程退出。
// Drop 中 close 是所有权语义的标准用法（参考 std::fs::File 的 Drop impl）。
```

---

### C-1-8 ~ 1-9 — POSIX libc（`py/runtime.rs:148/431`）

**C-1-8** `py/runtime.rs:148`（`is_live_group_leader`：判进程组 leader）
```rust
#[cfg(unix)]
pub fn is_live_group_leader(pid: u32) -> bool {
    unsafe { libc::getpgid(pid as i32) == pid as i32 }
}
```
**建议 SAFETY**：
```
// SAFETY: pid 由调用方传入（subprocess 子进程 PID），本函数语义：
// 若 getpgid(pid) == pid 则该 pid 是进程组 leader。
// POSIX getpgid(0) 返回调用方 PGID 是无害的（这里 pid 是入参，不会传 0）。
// 返回值仅用于 == 比较，无 wrapper 误用风险。
```

**C-1-9** `py/runtime.rs:431`（`pre_exec`：fork 后 exec 前设 RLIMIT）
```rust
unsafe {
    cmd.pre_exec(move || {
        let mem = libc::rlimit { rlim_cur: ..., rlim_max: ... };
        libc::setrlimit(libc::RLIMIT_AS, &mem);
        let cpu = libc::rlimit { ... };
        libc::setrlimit(libc::RLIMIT_CPU, &cpu);
        Ok(())
    });
}
```
**建议 SAFETY**：
```
// SAFETY: pre_exec 在 fork 后 exec 前运行，处于单线程上下文（POSIX 要求 async-signal-safe）。
// libc::setrlimit 对 RLIMIT_AS / RLIMIT_CPU 是 async-signal-safe 调用。
// mem_bytes / cpu_secs 来自 RunLimits，由调用方在子进程 spawn 前已 validate 非零。
// 命令行参数 process_group(0) 已将子进程设为新进程组，避免 setpgid 边界。
```

---

## C-2 — `app_state.rs:17-25` 总表行内的阶段号（弱建议）

**问题**：总表每行的 `**已迁入 ...（阶段 X.Y）**` 标记，半年后阶段号失去意义，但表的"快照"价值仍在。

**改写方向**：

| 原 | 建议 |
|----|------|
| `**已迁入 \`AppState.skill_runs\`**（阶段 3.3）` | `→ \`AppState.skill_runs\`` |

仅适用总表内部；模块顶部的 `//!` 注释中的阶段号已在 A 类阶段处理。

---

## C-3 — 复杂函数缺 why（**仅列示，未细查**）

无强制需要。**建议用户拍板**是否补充。

已知候选（粗扫）：
- `src-tauri/src/api_server.rs` 启动 / 端口绑定逻辑
- `src-tauri/src/migration/run.rs` 多步骤归档
- `src-tauri/src/bot_model_loop.rs` 流式对话驱动

待用户决定是否纳入下一轮 hygiene。

---

## C-4 — TS 导出函数缺 JSDoc

`src/` 仅 1 处日期戳命中（`format.ts:42`），且含 WHY 已保留。TS 侧基本干净，**无强制需要**。

---

## 应用方式（用户决定）

- C-1 一文件一 patch：每文件单独 `git apply`，便于回滚
- C-2 可并入 B 类
- C-3/C-4 暂缓

## 勘误说明（vs 上一版）

- 原 `C-1-4 ~ C-1-7` 误标为 pyO3 — 实际是 **Win32 Job Object FFI**
- 原 `C-1-8 ~ C-1-9` 误标为 pyO3 — 实际是 **POSIX libc** + `std::os::unix::process::CommandExt::pre_exec`
- 原 SAFETY 描述「`Python::with_gil` GIL 期间对象存活」对 Win32 / libc 调用完全不适用
- 勘误后的 SAFETY 直接引用 Win32 / POSIX 契约 + Rust 端不变性（valid HANDLE / size_of_val 校验 / async-signal-safe 等），不引外部文档