# tiny_http Vendor Patch 治理

> **目的**：记录 `src-tauri/vendor/tiny_http/` 这个 vendored crate 的 wmessage 自定义改动，
> 锁版本、锁改动点、锁 rebase 流程，避免 patch 漂移。

## 1. 上游版本

- crates.io 版本：`tiny_http = 0.12.0`
- vendor 来源：`src-tauri/vendor/tiny_http/Cargo.toml` 已写 `version = "0.12.0"`，与 crates.io 一致
- 本地引入 commit（首个 vendor + patch）：`10acf42`（`fix(audit): tiny_http 读超时 patch 堵 slowloris + MarkdownText 代码块误判修复`）

## 2. wmessage 自定义改动（共 3 处，均带 `[wmessage patch]` 标记）

### 2.1 `src/lib.rs`

- 行号范围：`~95-141`（`use std::sync::atomic::AtomicU64;` 引入 + `pub static HTTP_READ_TIMEOUT_MS` 定义 + 大段取舍注释）
- 关键符号：

```rust
// [wmessage patch] 批次4审计 P1-1：per-connection 读超时（见下方 HTTP_READ_TIMEOUT_MS）
use std::sync::atomic::AtomicU64;

...

/// [wmessage patch] 批次4审计 P1-1：per-connection 读超时（毫秒，默认 30s）。
/// accept 后对每个 TcpStream 调 `set_read_timeout`（见 connection.rs `Listener::accept`），
/// 堵 header 阶段 slowloris：原版 tiny_http 的 per-connection 读线程同步读完整个 header
/// 才产出 Request，无超时的慢速滴注连接会永久占住一个 TaskPool 线程（线程数无上限）。
/// ...
pub static HTTP_READ_TIMEOUT_MS: AtomicU64 = AtomicU64::new(30_000);
```

- 取舍（已写进上游注释，**不要重写**）：
  - 单次 read 系统调用级超时（非 header 总时长）
  - keep-alive 空闲超时会被断开（回 408，标准行为）
  - 只影响读，不影响写（SSE upgrade 后不受影响）
  - body 滴注由 wmessage 侧 `read_body_limited` 总时长 deadline（35s）补齐

### 2.2 `src/connection.rs`

- 行号范围：`~28-38`（`Listener::accept` 的 Tcp 分支内；AP-04 起含读+写两条 set_timeout）
- 关键改动：

```rust
pub(crate) fn accept(&self) -> std::io::Result<(Connection, Option<SocketAddr>)> {
    match self {
        Self::Tcp(l) => l.accept().map(|(conn, addr)| {
            // [wmessage patch] 批次4审计 P1-1：per-connection 读超时（slowloris 防护），
            // 语义与取值见 lib.rs `HTTP_READ_TIMEOUT_MS` 注释。失败忽略（不阻塞 accept）。
            let _ = conn.set_read_timeout(Some(std::time::Duration::from_millis(
                crate::HTTP_READ_TIMEOUT_MS.load(std::sync::atomic::Ordering::Relaxed),
            )));
            (Connection::from(conn), Some(addr))
        }),
        #[cfg(unix)]
        Self::Unix(l) => l.accept().map(|(conn, _)| (Connection::from(conn), None)),
    }
}
```

- Unix socket 分支保持原样（slowloris 威胁模型针对 TCP）

### 2.3 写超时 patch（2026-09-24 AP-04，跨 §2.1/§2.2 同两文件）

- `src/lib.rs`：新增 `pub static HTTP_WRITE_TIMEOUT_MS: AtomicU64 = AtomicU64::new(30_000)`
  （紧跟 `HTTP_READ_TIMEOUT_MS` 之后，带 `[wmessage patch]` 标记 + 取舍注释）；
  同时修订读超时注释尾弹——原「只影响读，不影响写：SSE 不受影响」自本 patch 起过时
  （读超时仍只管读；写侧由新 static 接管）。
- `src/connection.rs`：`Listener::accept` Tcp 分支既有 `set_read_timeout` 旁新增
  `set_write_timeout`（同 `[wmessage patch]` 标记、同失败忽略策略）。
- 目的：SSE writer（upgrade 后只写不读）对慢/半开对端的 `write_all` 可永久阻塞在
  syscall，stop 标志只在两次写之间轮询 → api_stop 停不掉写死的 writer，join 超时后
  handle detach、OS 线程泄漏（OCR C5-AP-04 sse.rs:196）。30s 单次 write syscall 级超时
  后写报错 → writer 退出可 join。普通 API 响应（小 JSON）写瞬时完成，无感知。
- Unix socket 分支不动（与读超时 patch 一致）。
- rebase 锚点：`grep "HTTP_WRITE_TIMEOUT_MS"`（lib.rs 定义 + connection.rs 调用）。

## 3. Rebase 步骤（升级 upstream 时）

```bash
# 1. 记下当前 wmessage patch 的精确改动（git 已追踪 vendor/tiny_http/，但忽略具体行）
git -C src-tauri/vendor/tiny_http log -1   # 当前 vendor 快照
git -C src-tauri/vendor/tiny_http diff HEAD -- src/lib.rs src/connection.rs > /tmp/wmsg-tiny_http.patch

# 2. 拉上游新版本（例 0.12.x → 0.13.0）
cd /tmp && cargo new tiny_http-fresh && cd tiny_http-fresh
# 拷入新版本 src/，保留我们两处 patch 的标记（grep "[wmessage patch]" 锚定）

# 3. 应用我们的 patch
patch -p1 < /tmp/wmsg-tiny_http.patch
# 若冲突：手解，必须保留 [wmessage patch] 注释与 `pub static HTTP_READ_TIMEOUT_MS`

# 4. 替换 vendor
rm -rf src-tauri/vendor/tiny_http/src
cp -R /tmp/tiny_http-fresh/src src-tauri/vendor/tiny_http/
# Cargo.toml 保留我们这版（不要覆盖）

# 5. 必跑验证（任何一步失败立即回滚）
cd src-tauri && cargo check
cd src-tauri && cargo test --lib --no-fail-fast    # 重点看 api_server::tests::slowloris_*
cd src-tauri && cargo clippy --all-targets -- -D warnings

# 6. 更新本文件：
#    - 改 §1 上游版本号
#    - 改 §2 行号范围（rebase 后行号会变）
#    - 追加 §5 升级记录
```

## 4. CI 守卫

- 脚本：`scripts/ci-guard-tiny-http-vendor.sh`
- 检查项：`cargo metadata --format-version 1` 输出里 tiny_http 的 `source` 必须指向
  本地 `vendor/tiny_http`，且与上游 0.12.0 一致
- 失败退出码 1，提示指向本文件

## 5. 升级记录

| 日期 | upstream | wmessage commit | 改动 |
|---|---|---|---|
| 2026-09-03 | tiny_http 0.12.0 | `10acf42` | 首次引入 vendor + 两处 patch（slowloris 防护） |
| 2026-09-14 | tiny_http 0.12.0 | `e1dd21e`（HEAD） | 仅文档治理（`PATCHES.md` + CI 守卫），未改 vendor 代码 |
| 2026-09-24 | tiny_http 0.12.0 | AP-04 批（本 commit） | 新增写超时 patch（§2.3）：lib.rs `HTTP_WRITE_TIMEOUT_MS` + connection.rs `set_write_timeout` |