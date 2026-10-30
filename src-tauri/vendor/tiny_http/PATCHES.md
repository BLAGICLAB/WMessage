# tiny_http Vendor Patch 治理

> **目的**：记录 `src-tauri/vendor/tiny_http/` 这个 vendored crate 的 wmessage 自定义改动，
> 锁版本、锁改动点、锁 rebase 流程，避免 patch 漂移。

## 1. 上游版本

- crates.io 版本：`tiny_http = 0.12.0`
- vendor 来源：`src-tauri/vendor/tiny_http/Cargo.toml` 已写 `version = "0.12.0"`，与 crates.io 一致
- 本地引入 commit（首个 vendor + patch）：`10acf42`（`fix(audit): tiny_http 读超时 patch 堵 slowloris + MarkdownText 代码块误判修复`）

## 2. wmessage 自定义改动（共 5 处，均带 `[wmessage patch]` 标记）

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

### 2.4 `src/client.rs`（2026-10-27 头部双上限）

- 引入 commit：`971d12d3`（`fix(security): 配置与 key 变更留审计、py 子进程环境改白名单、HTTP 头部加上限`）
- 关键符号：

```rust
// [wmessage patch] 2026-10-27：单条头（含请求行）的长度上限（字节）。
const MAX_HEADER_LINE_BYTES: usize = 16 * 1024;

// [wmessage patch] 2026-10-27：单个请求的头部条数上限。
const MAX_HEADER_COUNT: usize = 100;
```

- 改动点（同文件两处 + 文件尾 `mod test` 两条测试）：
  - `ClientConnection::read_next_line` 逐字节循环内加单行长度闸（`buf.len() > MAX_HEADER_LINE_BYTES`
    即按 `InvalidData` 断开）——防慢滴注客户端逐字节发一行永不换行的头把读缓冲无限撑爆；
  - header 解析循环入口加条数闸（`headers.len() >= MAX_HEADER_COUNT` 即按协议错误终止连接）
    ——防无限发头拖爆内存与解析时间。
- 与 §2.1 读超时、wmessage 侧 body 35s deadline 同属 slowloris 纵深防线。
- rebase 锚点：`grep "MAX_HEADER_LINE_BYTES"`（常量定义 + 两处闸 + 两条测试）。

### 2.5 `Cargo.toml`（2026-10-31 lint 消音表）

- 尾部追加：

```toml
# [wmessage patch] 2026-10-31：上游 0.12.0 原生 lint 噪音消音，登记见 PATCHES.md §2.5
[lints.rust]
unused_imports = "allow"
dead_code = "allow"
```

- 背景：上游 0.12.0 自带两条 lint 噪音——`util/mod.rs` 的 `SequentialWriter`
  重导出未被引用（unused_imports；类型本体在 client.rs 活着，死的只是重导出名）、
  `lib.rs` 的 `MustBeShareDummy` 是 Sync+Send 编译期断言 trait（dead_code 误报）。
  cargo 对 registry 依赖自动加 `--cap-lints allow`，对 path 依赖不加，故重编
  vendor 时会刷出这两条。
- 取舍：对**类**消音（非对点），比 cargo 给 registry 依赖的全类封顶还窄；代价是
  vendor 内这两类 lint 全静音——vendor patch 走 diff 人审 + 本文件登记，不依赖
  rustc 警告兜底。上游若未来版本自愈，可随升级撤掉本表。
- rebase：无需锚点——§3 第 4 步本就「Cargo.toml 保留我们版（不要覆盖）」，
  本表随我们版 manifest 自然存活。

## 3. Rebase 步骤（升级 upstream 时）

```bash
# 1. 记下当前 wmessage patch 的精确改动（git 已追踪 vendor/tiny_http/，但忽略具体行）
git -C src-tauri/vendor/tiny_http log -1   # 当前 vendor 快照
git -C src-tauri/vendor/tiny_http diff HEAD -- src/lib.rs src/connection.rs src/client.rs > /tmp/wmsg-tiny_http.patch

# 2. 拉上游新版本（例 0.12.x → 0.13.0）
cd /tmp && cargo new tiny_http-fresh && cd tiny_http-fresh
# 拷入新版本 src/，保留我们 patch 的标记（grep "[wmessage patch]" 锚定）

# 3. 应用我们的 patch
patch -p1 < /tmp/wmsg-tiny_http.patch
# 若冲突：手解，必须保留 [wmessage patch] 注释与 `pub static HTTP_READ_TIMEOUT_MS` / `HTTP_WRITE_TIMEOUT_MS` / client.rs 两个上限常量

# 4. 替换 vendor
rm -rf src-tauri/vendor/tiny_http/src
cp -R /tmp/tiny_http-fresh/src src-tauri/vendor/tiny_http/
# Cargo.toml 保留我们这版（不要覆盖；含 §2.5 lint 消音表）

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
| 2026-10-27 | tiny_http 0.12.0 | `971d12d3` | 新增头部双上限 patch（§2.4）：client.rs 单行 16KB / 条数 100（慢滴注防线）；当日漏记本表与 §2，2026-10-31 补登记并补源码 `[wmessage patch]` 标记 |
| 2026-10-31 | tiny_http 0.12.0 | 本批 | Cargo.toml 加 `[lints.rust]` 两类 allow 消上游原生 lint 噪音（§2.5）；同批补登记 971d12d3（§2.4） |