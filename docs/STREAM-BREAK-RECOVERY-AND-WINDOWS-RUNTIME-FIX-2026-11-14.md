# Windows 流式断流「内部错误」修复方案

> **状态：已实施（0cefe322..169a660b 六批，2026-11-14 落地；实机 M197/M199/M230 见 11-18-full）。** 2026-11-14 收到 Windows 用户实机日志（`error.log` + `bot.log` 截断样本，
> 2026-11-13 11:23–16:19），诊断结论是「网络层响应体截断」被本仓三层设计叠加放大成不可诊断、
> 不可恢复、不可重试的「内部错误」。本文是细化方案：现状事实（带代码坐标）、业界口径、
> 设计、锚点、批次切分、不做清单、待拍板。
>
> 上游诊断记录见当日会话；本文只放落地必需的部分。冲突时以本文为准。

## 〇、总览

| # | 项 | 一句话 | 性质 | 批次 |
|---|---|---|---|---|
| F-1 | 断流错误分类 | 读侧断流/idle 超时改报 `LlmRequestFailed`（可恢复），不再 blanket `Internal` | 修分类 | 批 2 |
| F-2 | 断流文案分层 | 原始 reqwest 错误串只进 audit；用户气泡只给人话 + 下一步 | 修文案 | 批 2 |
| F-3 | 诊断留痕补全 | `llm.stream_failed` 记 source chain / 已收字节 / 阶段 / 耗时 | 补可观测 | 批 1 |
| F-4 | 重发判据修正 | 思考增量不再毒化 `emitted_any`；新增 `bot-turn-restart` 让本轮干净重来 | 修判据 | 批 3 |
| F-5 | 失败气泡重试入口 | failed 气泡下给「🔁 重试」按钮（重发其前最近一条 user 消息） | 新增 UX | 批 4 |
| F-6 | ort 加载降级 | dll 预检 + `catch_unwind` + 永久降级；记忆/OCR 不再 panic 毒化全局 | 修降级 | 批 5 |
| F-7 | Windows 打包补件 | installer 补 MSVC runtime 或启动自检提示 | 修打包 | 批 6 |

批次顺序与依赖见 §五；不做清单 §六；待拍板 §七；门禁影响 §八；风险与偏离登记 §九；实机复验 §十。

**外部依赖**：批 3 待 T-1 拍板，批 4 待 T-2 拍板，批 6 待 T-4 拍板。
**批 1 / 批 2 / 批 5 无外部依赖，可立即开工。**

---

## 一、现状事实

### 1.1 症状落点：用户看到的「内部错误」是怎么拼出来的

实机样本（用户气泡内容）：

```
⚠️ 内部错误：流式读取失败：error decoding response body
```

拼接链（逐段有代码坐标）：

1. `src-tauri/src/bot_model_loop/sse.rs:493-499` —— 读侧出口把 reqwest 错误包成
   `CommandError::Internal(format!("流式读取失败：{e}"))`。
2. `src-tauri/src/error.rs:391` —— `Self::Internal(s) => format!("内部错误：{s}")`。
3. `src-tauri/src/error.rs:299` —— `Self::Internal(_) => false`，即 **recoverable=false**。
4. `src-tauri/src/bot_chat.rs:302-324` `build_failed_row` —— 失败行内容 =
   `format!("{partial}\n\n⚠️ {}", e.message())`，`turn_status = "failed"`，**落库**，
   经 `bot-msg-upserted` 事件投影到前端。
5. `src/components/ChatPanel/MessageList.tsx:136-141` —— failed 气泡走普通 markdown 渲染，
   **没有任何重试入口**；`liveTurn` 判据（:128-133）把 `failed` 排除在活体指示外。

**结论**：用户看到的「内部错误」是**聊天气泡里的一行普通文本**，不是弹窗。
`recoverable=false` 在这条路径上其实**不参与**（前端 `handleCommandError` 的重试按钮只对
`invoke` 抛出的错误生效，而本仓失败行已先落库 + 事件投影）。这纠正了一个容易走偏的判断：
**光把 `Internal` 换成 `LlmApiError` 并不会凭空长出「重试」按钮**——按钮得由前端单独做（F-5）。

### 1.2 读侧断流的两个出口与它们的分类

`src-tauri/src/bot_model_loop/sse.rs`：

| 出口 | 位置 | 现状错误 | retryable | 用户可见文案 |
|---|---|---|---|---|
| chunk 间 idle 超时（120s） | :466-482 | `CommandError::Internal` | true | 内部错误：流式读取超时：… |
| 连接中断 / body 解码失败 | :487-500 | `CommandError::Internal` | true | 内部错误：流式读取失败：… |

对照**流内错误**（上游 200 流里带 error 载荷）的出口 `sse.rs:704-718`：走
`stream_error_to_cmd_error` → `CommandError::LlmApiError { status, detail }`，
`is_recoverable=true`（`error.rs:295`），且带按状态码的人话指引。

同文件 `sse.rs:107-125` 的注释把这条差距写得很明白：

> 分类与 HTTP 路径对齐：一律报 `LlmApiError`（recoverable=true，前端给「重试」入口 + 按状态码的人话指引），
> **不再降级成 Internal**——Internal 恒为 recoverable=false，前端只会让用户「反馈日志」。

**读侧两条出口没跟上这次改造**——不是设计分歧，是漏改。

### 1.3 重发判据为什么在「开着思考」时恒不成立

`src-tauri/src/bot_model_loop/core.rs:344`（读侧）与 `:375`（收尾侧）共用：

```rust
if fail.retryable && !fail.emitted_any && stream_retry < MAX_STREAM_RETRIES
```

`emitted_any` 由 `sse.rs:408-424` 的 `emit` 闭包置位。**闭包不区分事件类型**——
`handle_line` 里三类事件都走它：

| 事件 | 位置 | 语义 | 是否该算「用户已看到内容」 |
|---|---|---|---|
| `bot-think-delta` | `sse.rs:316-319` | 思考增量 | **否**（思考是过程，不是答复正文） |
| `bot-chat-delta` | `sse.rs:337` | 正文增量 | **是**（重发＝用户看两遍正文） |
| `bot-tool` / `bot-tool-name` | `sse.rs:366-378` | 工具折叠行 | 是（视觉重复） |

实机配置：每一行 `llm.request` 都是 `reasoning=thinking.budget_tokens=4096`，
即**思考始终开着**。于是**第一个 thinking delta 就把 `emitted_any` 置成 true**，
`!fail.emitted_any` 恒假 → `llm.stream_retry` 这条通道在本机配置下**几乎是死代码**
（`MAX_STREAM_RETRIES = 1`，`request.rs:34`）。

后果：一次断流 = 整轮作废。实机样本里最痛的一例是
`sa_9075e1b4e23042a698836ae2e2cffc18`：跑了 257995ms、7 个子任务、大量正文后断流，
`error=内部错误：流式读取失败：error decoding response body`，全部白干。

**安全性核对（本方案的前提，必须先确认）**：断流时本轮的 `tool_calls` **尚未执行**——
工具执行发生在 `stream_round` 成功之后。所以「重发本轮」不会重复执行工具副作用；
历史轮的工具结果已在 `msgs` 里，重发不会让它们重跑。**重发本轮是安全的。**

### 1.4 诊断信息在哪一步被丢掉

`error decoding response body` 是 reqwest 自己的包装串：

- `reqwest-0.13.4/src/error.rs:242` → `Kind::Decode => f.write_str("error decoding response body")`；
- `reqwest-0.13.4/src/async_impl/response.rs:352` → `bytes_stream()` 把 hyper 的 body 错误
  一律 `map_err(crate::error::decode)`。

**真正的 hyper 错误在 `reqwest::Error` 的 `source()` 链里**（`connection closed before message
completed` / h2 `RST_STREAM` / chunked 分块截断 / TLS 中断…），而我们的审计只记
`e.to_string()`（`sse.rs:491`）。

→ **这个错误从设计上就永远查不出真因**。用户日志里 6 次断流，6 次都是同一句废话，
无法区分「上游断链」「代理截断」「TLS 中断」「h2 reset」。

### 1.5 Windows 专属 A：`onnxruntime.dll` 加载失败 → ort panic → 全局 Mutex 中毒

实机样本（应用启动后 40 秒内）：

```
ERROR | panic | thread=<unnamed> | loc=ort-2.0.0-rc.13/src/lib.rs:234
  | msg=Failed to load ONNX Runtime dylib: Dlopen { error: LoadLibraryExW { source: 126 },
  | path: "C:\Users\PC\AppData\Local\wmessage\onnxruntime.dll" }
ERROR | panic | thread=tokio-rt-worker | loc=ort-2.0.0-rc.13/src/util/mutex_std.rs:15 | msg=Mutex poisoned
WARN  | memory.injection_failed | error=记忆检索线程 join 失败：task 953 panicked with message "Mutex poisoned"
```

链路（逐段有坐标）：

1. `ort-2.0.0-rc.13/src/lib.rs:234` —— `load_dynamic::init(&path).expect("Failed to load ONNX Runtime dylib")`。
   **是 panic，不是 Err**——`expect` 遇错直接炸。
2. panic 发生在持锁状态 → ort 自己的 `Mutex` 被毒化
   （`ort-2.0.0-rc.13/src/util/mutex_std.rs:15`：`Err(_) => panic!("Mutex poisoned")`）。
3. 此后**每一次** ort 调用都 panic。记忆侧表现：
   `src-tauri/src/memory/mod.rs:387`（`memory.injection_failed`）、`:617`（`recall_facts` 工具失败）、
   `:739` / `:794`（`memory.lesson_save_failed`）。实机日志里这两类 WARN 反复出现。

**这违反了本仓自己写下的契约**——`src-tauri/src/memory/embed.rs:5-7`：

> 加载失败把原因缓存下来并永久返回 None —— 全系统优雅降级为纯关键词模式，
> **绝不让聊天/写入路径因模型缺失而失败**。

契约只在「ort 返回 Err」时成立；ort 实际是 panic，所以契约**从未兑现**。

**为什么是 126**：`LoadLibraryExW` 的 126 = `ERROR_MOD_NOT_FOUND`，两种成因——
(a) 文件不在；(b) 文件在但**依赖 dll 缺失**。

读 `ort-2.0.0-rc.13/src/lib.rs:126-135` 的解析规则可以把两者分开：

```rust
let absolute_path = if path.is_absolute() {
    path.to_path_buf()
} else {
    let relative = std::env::current_exe()?.parent()?.join(path);   // path = "onnxruntime.dll"
    if relative.exists() { relative } else { path.to_path_buf() }  // ← 存在才给绝对路径
};
```

默认路径来自 `lib.rs:224-232`：`ORT_DYLIB_PATH` 环境变量优先，否则 Windows 恒为
`"onnxruntime.dll"`（相对名）。

**日志里的路径是绝对路径**（`C:\Users\PC\AppData\Local\wmessage\onnxruntime.dll`）——
而相对名只有在 `relative.exists()` 为真时才会被提升成绝对路径。
本仓**任何地方都不设 `ORT_DYLIB_PATH`**（installer / 启动脚本 / 代码均无），
故走的是默认分支 → **`relative.exists()` 为真 = 文件就在安装目录里**。

→ **结论收紧：dll 装了，126 是「依赖缺失」，不是「文件缺失」。**
（若目标机确实被人手设过 `ORT_DYLIB_PATH`，需按 §十 W-5 现场复核一次；默认路径下结论如上。）

核 `src-tauri/bundle-staging/onnxruntime.dll` 的 PE 导入表，它依赖：

```
MSVCP140.dll   MSVCP140_1.dll   VCRUNTIME140.dll   VCRUNTIME140_1.dll
```

这四个是 **MSVC 运行库**。NuGet 的 `microsoft.ml.onnxruntime` 包**不含**它们，
`src-tauri/tauri.windows.conf.json` 的 resources 映射里也没有：

```json
"bundle-staging/onnxruntime.dll": "onnxruntime.dll",
"bundle-staging/onnxruntime_providers_shared.dll": "onnxruntime_providers_shared.dll"
```

→ **目标机没装 VC++ 2015-2022 x64 运行库时，dll 就算躺在安装目录里也照样 126。**

仓库文档其实已经知道这条（`docs/WINDOWS-INSTALLER-DESIGN-2026-11-05.md:46`）：
「Windows 目标 ort 是 load-dynamic：构建期不需要 onnxruntime 文件，**运行期缺 dll 会 panic 不降级**」
——**只记录，没做防护**。

资源映射本身核对过是对的：`tauri-utils-2.10.1/src/resources.rs:205-251` 的 map 分支，
单文件 source + 非空 dest → `target = dest.clone()`，落点是 `$RESOURCE/onnxruntime.dll`
= `%LOCALAPPDATA%\wmessage\onnxruntime.dll`，与日志里的路径一致。
**不是映射 bug，是依赖缺失。**

### 1.6 Windows 专属 B：reqwest 默认读系统代理

`reqwest-0.13.4/Cargo.toml` 的 `default = ["default-tls", "charset", "http2", "system-proxy"]`，
`reqwest-0.13.4/src/async_impl/client.rs:416` → `proxies.push(ProxyMatcher::system())`。

→ **Windows 上 reqwest 会读 WinINET 系统代理**。目标机若装了系统代理（企业/校园网常见），
LLM 流全部经过中间件，截断概率显著高于 macOS。

本仓**没有**任何代理配置面（`shared_llm_client()` 见 `bot_model_loop/shell.rs:100-108`，
只有 `connect_timeout(15s)`），也**没有**暴露开关。

> **本方案不为此加开关**（见 §六 第 5 条）：先用批 1 的 `err_chain` 确认目标机是否真被
> 中间件截断，确认了再单独立项。**这是本方案里被显式推迟的一条，不是遗漏。**

### 1.7 用户可见的次生伤害：失败后立刻重发会被防重入拦掉

实机日志里两次断流的**同一毫秒**都跟着：

```
[15:01:49.323] INFO | bot_chat_rejected | detail=session: 36ed… || 已有执行实例在跑（防重入拦截）
```

用户在气泡里看到「内部错误」后马上重发，消息被会话锁拦掉——失败与重试之间没有任何缓冲。

---

## 二、业界口径

| 来源 | 口径 | 对本仓的含义 |
|---|---|---|
| Codex（`stream disconnected before completion`） | 默认重试 **5 次**；错误后缀是诊断本身（server / timeout / proxy / websocket） | 断流不该是终局；错误文案要指向成因 |
| Codex 根因四分类 | ① Windows TCP 终止空闲连接（os error 10053）② 远程压缩失败 ③ 模型绑定 ④ 本地中间件干扰 | Windows 侧优先怀疑 TCP 空闲与中间件 |
| codewhale-hq/Codewhale#103 | 同一串错误定位为 chunked 截断；修复是「记完整 reqwest source-chain + headers + 已收字节」+「没吐内容才重试」 | 与 F-3 / F-4 同构；「记 source chain」是业界标准做法 |
| everruns-provider `stream_reconnect` | `is_reconnectable_reqwest_error`：body/decode 错误（截断/半截 body/中途 reset）**属可重连**；stale 池连接、首个字节前读超时同属 | 断流 = 可重连类错误，不是确定性失败 |
| Claude Code / Codex 失败 UX | 给「重试」而不是「反馈日志」 | F-5 |

---

## 三、设计

### 3.1 F-3｜诊断留痕补全（批 1，纯审计，零行为变化）

改 `sse.rs` 读侧 `Some(Err(e))` 分支：

```rust
Some(Err(e)) => {
    audit(
        crate::audit::AuditLevel::Error,
        "llm.stream_failed",
        vec![
            ("err", e.to_string()),                 // 保留：reqwest 包装串
            ("err_chain", err_chain(&e)),           // 新增：source 链逐层
            ("bytes", bytes_received.to_string()),  // 新增：已收字节数
            ("elapsed_ms", elapsed_ms.to_string()), // 新增：响应头→断流耗时
            ("phase", "body".to_string()),          // 新增：阶段（body）
        ],
    );
    ...
}
```

- `err_chain(e)`：新纯函数，逐层 `std::error::Error::source()`，用 ` ← ` 连接，截断到 400 字符。
  **这是把「永远查不出真因」变成「查得出」的那一刀。**
- `bytes_received`：`read_sse_stream` 里新增累计计数（`byte_buf` 会被 drain 掉，不能拿它当数）。
- `elapsed_ms`：从 `resp` 拿到（响应头已到）到断流的时长。

**为什么先做这一批**：F-1~F-7 的验证都依赖「能看出断在哪」。先做可观测，
后面每一批的实机复验才有判据。

### 3.2 F-1 + F-2｜错误分类与文案分层（批 2）

**F-1 分类**：读侧两条出口（idle 超时 / 连接中断）从 `CommandError::Internal` 改为
`CommandError::LlmRequestFailed`——`error.rs:362` 渲染为 `请求大模型失败：{s}`，
`error.rs:294` `is_recoverable = true`。

**为什么不新增变体**：新增 `CommandError` 变体要同步三处（`error.rs` 枚举 + `error.rs` 的
`code()`/`is_recoverable()` + `src/lib/errorHandler.ts` 的联合类型与 `ALL_COMMAND_ERROR_CODES`），
门禁 `tests-audit/audit_error_codes.py` 强制跨语言一致。**复用 `LlmRequestFailed` 零改动过闸。**

**F-2 文案分层**：用户气泡里**不出现** `error decoding response body`（对用户无意义、
且泄露内部实现），也不出现「内部错误」这个把用户引向报障而非自救的词。

用户可见 note（新构造，`sse.rs` 内）：

```
网络连接中断，本轮未完成（已自动重试 1 次仍失败）。点气泡下方的「🔁 重试」可重新发起本轮。
```

- 原始 reqwest 错误串**只进 audit**（F-3 已保证有 source chain），不进 `message()`。
- 文案随「是否已自动重试过」变：没重试过 → 「网络连接中断，本轮未完成」；重试过 → 追加
  「已自动重试 1 次仍失败」。
- idle 超时单独一档文案：「大模型长时间没有响应，本轮未完成……」。

**F-5 落地后**，这个 note 后面跟一个按钮；F-5 未落地时，note 里的「点气泡下方的『🔁 重试』」
不能出现——**批 2 与批 4 有文案耦合**，批 2 先按「暂不提按钮」版本落，批 4 再补那句。
（这一条要在批 4 的 diff 里同步改回。）

### 3.3 F-4｜重发判据修正 + 本轮重置（批 3，依赖 T-1）

**判据拆分**：`emitted_any: AtomicBool` → 两个：

- `emitted_body`：正文（`bot-chat-delta`）+ 工具折叠行（`bot-tool` / `bot-tool-name`）；
- `emitted_thinking`：仅 `bot-think-delta`。

`emit` 闭包按事件名分派置位。`StreamFailure.emitted_any` 字段随之改为 `emitted_body`
（`sse.rs:77-84` + `core.rs:344/375` 同步改）。

**重发条件**：`fail.retryable && !fail.emitted_body && stream_retry < MAX_STREAM_RETRIES`
——**只吐过思考不算「已吐内容」，允许重发**。

**本轮重置（新事件 `bot-turn-restart`）**：重发前发一次，前端清掉本轮已出的思考与工具行，
否则用户会看到两段思考（思考是**追加**的：`src/components/ChatPanel/useChatStream.ts:148`
`m.thinking = (m.thinking ?? "") + t`）。

- **后端**：`core.rs` 重发分支里，重发前 `emit("bot-turn-restart", json!({ "sessionId": sid }))`。
- **记账**：`src-tauri/src/bot/turn_record.rs:50` `push_event` 增 `bot-turn-restart` 分支 →
  `rec["thinking"] = ""`、`rec["tools"] = []`。**不改会落库成两段思考拼接。**
- **前端**：`useChatStream.ts` 监听 `bot-turn-restart` → 清 `streamState.metaOf(sid).thinking`
  与工具行；清完继续等新流。
- **安全性**：清工具行是对的——断流时本轮工具**尚未执行**（见 §1.3），折叠行本来就是空壳；
  历史轮工具已执行、结果在 `msgs` 里，不受影响。

**边界**：`bot-turn-restart` 只在「重发本轮」时发，不在用户主动停止时发。

### 3.4 F-5｜失败气泡的重试入口（批 4，依赖 T-2）

**后端零改动**：不新增 DB 字段、不改 `build_failed_row`、不加新命令。

**前端**：`MessageList.tsx` 渲染 `turnStatus === "failed"` 的 assistant 气泡时，在气泡下方
加一行「🔁 重试」按钮。点击行为 = **重发该气泡之前最近的一条 user 消息**。

- 定位：从消息列表里往前找最近的 `role === "user"` 的行，取其 `content`。
- 复用 `useChatSend.ts` 现有的发送路径（不新造发送逻辑）。
- 语义说明：这是「重问一次」，不是精确的「重发本轮」。**精确重发需要回合级 checkpoint
  （`docs/AGENT-GAP-CLOSURE-DESIGN-2026-11-07.md` D-1，尚未实施），本方案不重复造。**
- 边界：没有前置 user 消息（理论上不会出现）→ 不渲染按钮。

### 3.5 F-6｜ort 加载降级（批 5）

**前提（已核实的两条事实）**：

1. `ort` **没有**公开的「可失败」的 dylib 初始化入口——`setup_api()` 是私有 `#[cold] fn`，
   内部 `load_dynamic::init(&path).expect("Failed to load ONNX Runtime dylib")`
   （`ort-2.0.0-rc.13/src/lib.rs:234`）。公开的 `ort::api()` 走
   `G_ORT_API.get_or_init(setup_api)` → 加载失败必然 panic。
2. `G_ORT_API` 是 `OnceLock`，**init closure panic 后保持未初始化**，下次调用会**重新执行 init**
   → 不做短路的话，**每一次**记忆检索都会再炸一次 panic。

**三道防线，从外到内**：

1. **真加载自检（消掉绝大多数 panic，且零噪音）**
   与其猜「文件在不在」，不如**按 ort 的同一套解析规则把 dll 真加载一次**：
   - 解析路径：复刻 `lib.rs:126-135`（`ORT_DYLIB_PATH` → exe 同目录 → 原相对名）；
   - 加载：Windows 下用 `windows::Win32::System::LibraryLoader::LoadLibraryExW`
     （本仓已有 `windows = "0.61"` 依赖，只需在 `Cargo.toml` 的 features 里加
     `Win32_System_LibraryLoader`——**不是新依赖**）；
   - 失败 → **拿到 Win32 错误码，不 panic**：
     - `126` → 「缺 MSVC 运行库（VCRUNTIME140 / MSVCP140 系）」；
     - `2` → 「onnxruntime.dll 未随包分发」。
   - 成功 → `FreeLibrary` 释放（让 ort 走自己的加载路径）。
   → **这一步同时产出了 T-4 自检需要的诊断信息**，一次实现两处受益。

2. **兜底（`catch_unwind`）**：自检通过但 ort 内部仍 panic 的残余路径
   （`MissingApi` 的 `.expect()`、`assert!(!base.is_null())`、版本不匹配等）：
   `load_engine()` 外包 `std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ...))`，
   panic → 转 `Err`。

3. **永久降级（不再重试）**：新增 `static ENGINE_FAILED: AtomicBool`——
   一旦失败就永久短路，后续直接返回 `Err("嵌入引擎不可用（已永久降级为关键词模式）")`，
   **不再触碰 ort**（否则 `OnceLock` 会无限重试 → 无限 panic + 无限毒化）。
   - **两侧都要做**（`memory/embed.rs` 与 `ocr.rs`）：ort 的全局 Mutex 被毒化后，
     OCR 侧第一次调用同样会 panic。

**审计**：加载失败记一条 `memory.embed_engine_unavailable`（WARN，含 Win32 错误码、降级结论、
以及「缺 MSVC 运行库」的明确指引），**只记一次**（现在是 `eprintln!`，没有审计事件，日志里查不到）。

**已知代价（不做）**：第 2 道防线拦下的残余 panic 仍会被 Rust 默认 panic hook 打到 stderr，
噪音保留。**接受，不改全局 panic hook**（改 hook 是全局副作用，多线程下有竞态，
代价大于收益）。第 1 道防线把主因（依赖缺失）挡在 panic 之前，噪音基本消失。

**顺带**：`ocr.rs` 的 `new_session` 是同一套 ort 调用，同样要包；两侧共享「永久降级」的语义
（ort 的全局 Mutex 是一份）。

### 3.6 F-7｜Windows 打包补件（批 6，依赖 T-4）

两条路，见 §七 T-4：

- **A 随包发**：staging 脚本额外抓 MSVC 运行库 4 个 dll 随安装包分发
  （许可可再分发，需确认来源与版本口径）。
- **B 启动自检**：首次启动用 §3.5 第 1 道防线的同一段代码试加载，失败 → 一次性提示
  「语义记忆不可用（缺 VC++ 2015-2022 x64 运行库），请安装 xxx」，其余功能照常。

**F-6 与 F-7 是互补的**：F-6 保证「缺件时优雅降级、不崩」，F-7 保证「缺件时用户知道怎么办」。
**只做 F-6 的话，用户看到的是「记忆功能静默失效」——比 panic 好看，但仍然没人知道为什么。**

---

## 四、锚点

### 4.1 后端（Rust）

| # | 锚点 | 落点 |
|---|---|---|
| A-1 | 断流时 `llm.stream_failed` 审计含 `err_chain`（含 hyper 原文）、`bytes`、`elapsed_ms`、`phase` | `src-tauri/tests/llm_integration/stream_failure.rs` + `tests/mock_llm.rs` 的 `TruncatedStream` |
| A-2 | 断流错误**不再**是 `CommandError::Internal`；`is_recoverable() == true` | 同上（断言错误类型 + `is_recoverable`） |
| A-3 | 用户可见文案**不含** `error decoding response body`、**不含**「内部错误」 | 同上（断言 `err.message()`） |
| A-4 | 只吐思考就断流 → **重发本轮**（请求数 = 2） | 新增用例：mock 先发 thinking_delta 再断 |
| A-5 | 已吐正文就断流 → **不重发**（请求数 = 1，请求数断言保持） | 现有 `stream_failure_after_output_does_not_retry` 语义调整 |
| A-6 | 重发前发 `bot-turn-restart`；`turn_record` 的 thinking/tools 被清空（落库不出现两段思考） | 新增用例 |
| A-7 | ort 加载失败 → 返回 `None`/`Err`，**不 panic**；第二次调用不再触碰 ort | `src-tauri/src/memory/embed.rs` 内的单测（纯函数 + 注入失败） |
| A-8 | `MAX_STREAM_RETRIES` 挪进 `bot/constants.rs` 后与 SOP 基线表一致 | `tests-audit/audit_sop_baseline.py` 门禁自绿 |
| A-9 | 真加载自检与 ort 的路径解析规则同源（`ORT_DYLIB_PATH` → exe 同目录 → 原相对名） | `memory/embed.rs` 内纯函数单测（路径解析抽成纯函数，可直测） |

### 4.2 前端（TS）

| # | 锚点 | 落点 |
|---|---|---|
| B-1 | failed 气泡下方渲染「🔁 重试」按钮 | `src/components/ChatPanel/MessageList.test.tsx` |
| B-2 | 点按钮 → 重发其前最近一条 user 消息（不新造发送路径） | 同上 |
| B-3 | 收到 `bot-turn-restart` → 本轮 thinking / 工具行被清空 | `src/components/ChatPanel/ChatPanel.stream.test.tsx` |
| B-4 | 没有前置 user 消息 → 不渲染按钮 | 同上 |

### 4.3 打包

| # | 锚点 | 落点 |
|---|---|---|
| C-1 | staging 完整性校验覆盖新增件（若走 T-4=A） | `scripts/stage-windows-bundle.sh` 的第 3 步 |
| C-2 | 安装目录里的 `onnxruntime.dll` 能被加载（自检脚本） | Windows 实机（§十） |

---

## 五、批次切分与顺序

| 批 | 内容 | 依赖 | 完成判据 |
|---|---|---|---|
| 1 | **F-3 诊断留痕** | 无 | A-1；`llm.stream_failed` 能看出断在哪 |
| 2 | **F-1 + F-2 分类与文案** | 无（文案按「暂不提按钮」版） | A-2、A-3；用户气泡不再出现「内部错误」 |
| 3 | **F-4 重发判据 + `bot-turn-restart`** | T-1 | A-4、A-5、A-6、B-3 |
| 4 | **F-5 失败气泡重试入口** | T-2；文案回填「点 🔁 重试」那句 | B-1、B-2、B-4 |
| 5 | **F-6 ort 加载降级** | 无（前置：读 ort `load_dynamic` 源码） | A-7；Windows 上不再有 Mutex poisoned 风暴 |
| 6 | **F-7 打包补件 / 自检** | T-4 | C-1 / C-2 |

**串行理由**：批 1 的审计字段是批 2~4 的实机判据（不先修可观测，后面的修复无法验证）；
批 2 与批 4 有文案耦合，必须按序；批 3 的 `bot-turn-restart` 与批 4 的按钮都在前端，
放一起能省一轮前后端联调验收。

**每批一个逻辑变更一个 commit**（`AGENTS.md` 工作方式）。

---

## 六、不做清单（本方案内明确不做）

1. **不做精确的「重发本轮」**——需要回合级 checkpoint + 回合重放，依赖
   `docs/AGENT-GAP-CLOSURE-DESIGN-2026-11-07.md` D-1（未实施）。F-5 的「重问一次」够用。
2. **不做断流后的自动续跑（continue generation）**——Anthropic 的 assistant prefill 续写只适用
   于纯文本轮；工具调用轮续跑会重复执行副作用。风险高于收益。
3. **不改全局 panic hook**（§3.5 已述理由）。
4. **不新增 `CommandError` 变体**——复用 `LlmRequestFailed`，零跨语言改动（§3.2）。
5. **不新增「LLM 请求不走系统代理」配置开关**——AGENTS.md「不加投机抽象：第二个调用方出现前
   不抽象」。现状（走系统代理）对多数用户是对的；先用批 1 的诊断确认目标机是否真被中间件截断，
   再单独立项。**这是本方案里被显式推迟的一条。**
6. **不引入 HTTP/1.1 强制降级开关**——同类问题业界用 `DEEPSEEK_FORCE_HTTP1=1` 这类 env 逃生口，
   但那是给「已确诊 h2 问题」的临时手段；现在还没确诊。先靠诊断。
7. **不改 `MAX_STREAM_RETRIES` 的数值**——保持 1（每次重发都是一次真实计费请求）。
   要不要提到 2 见 T-5。
8. **不动 `build_failed_row` 的落库格式**——`{partial}\n\n⚠️ {note}` 已被「失败叙事重启后从库还原」
   依赖（F-5 是纯前端，不碰它）。

---

## 七、待拍板

| # | 议题 | 选项 | 推荐 |
|---|---|---|---|
| T-1 | 思考增量算不算「已吐内容」？ | A：不算（只吐思考允许重发本轮，需 `bot-turn-restart` 清场）<br>B：算（维持现状，开着思考时永不重发） | **A**——开着思考是本仓默认配置，B 等于把重发通道整个废掉 |
| T-2 | 失败气泡要不要「🔁 重试」按钮？ | A：要（重发其前最近一条 user 消息，零后端改动）<br>B：不要，只改文案 | **A**——用户唯一的自助手段；实现成本最低 |
| T-3 | 断流重试次数 | A：保持 1<br>B：提到 2 | **A**——重发是真实计费请求；先看批 1 诊断出来的断流频次再定 |
| T-4 | MSVC runtime 怎么解决？ | A：随包发 4 个 dll（需确认许可与版本口径）<br>B：installer 启动自检 + 提示用户手动装 redist<br>C：只做 F-6 优雅降级，不补件 | **B**——不碰第三方 dll 的再分发许可；且 §1.5 已定位成「dll 在、依赖缺」，自检正好能报出准确的 126 与修复动作 |
| T-5 | 断流错误用哪个变体？ | A：复用 `LlmRequestFailed`（零跨语言改动）<br>B：新增 `LlmStreamInterrupted`（语义更准，要同步 Rust+TS 三处） | **A**——`LlmRequestFailed` 的语义（「请求没打通」）与断流吻合 |

---

## 八、门禁影响清单

| 门禁 | 受影响的批 | 要做什么 |
|---|---|---|
| `tests-audit/audit_error_codes.py` | 2 | 复用现有变体 → 无需改；若选 T-5=B 则要同步 Rust 枚举 + TS 联合类型 + `ALL_COMMAND_ERROR_CODES` |
| `tests-audit/audit_sop_baseline.py` | 3 | `MAX_STREAM_RETRIES` 挪进 `bot/constants.rs` → **必须同步 `docs/agent-audit-sop-zcode.md` 的基线常量表**（改风控常量不同步表 → 门禁红） |
| `tests-audit/audit_doc_claims.py` | 3 | 新事件名 `bot-turn-restart` 要在源码里真存在；引用它的文档要在 `DOCS` 列表或 `docs/acceptance/cards-*.md` 内 |
| `docs/acceptance/event-names.md` | 3 | 登记 `bot-turn-restart` |
| `tests-audit/audit_acceptance_matrix.py` | 2、4 | 批 2/4 是用户可见行为变更 → 同步 `SPEC.md` 功能条目 + `docs/acceptance/matrix.csv` + 对应 `cards-*.md` |
| `tests-audit/audit_file_size.py` | 5 | `memory/embed.rs` / `ocr.rs` 改动后仍须 ≤1500 行 |
| `tests-audit/audit_module_map.py` | 5 | 若 `memory/embed.rs` 拆出新文件，要同步 `docs/rust-bot-architecture.md` |
| `cargo clippy -D warnings` / `cargo nextest` | 全部 | 每批跑 `bash scripts/test-fast.sh` |
| `Cargo.toml` feature（非新依赖） | 5 | `windows` 加 `Win32_System_LibraryLoader` feature（`src-tauri/Cargo.toml:190-205` 那段） |
| `tests-audit/audit_doc_claims.py` | 5 之后 | 批 5 落地后把本文加入该脚本的 `DOCS` 列表——本文引用了 `memory.embed_engine_unavailable` 等事件名，源码里有了才纳入对拍（现在纳入会红） |

---

## 九、风险与偏离登记（实施时回填）

| 风险 | 说明 | 缓解 |
|---|---|---|
| `catch_unwind` 拦不住 abort | 原担心 ort 在加载路径里调 `std::process::abort` | **已排除**：读过 `ort-2.0.0-rc.13/src/lib.rs:122-171` 与 `:215-250`，加载路径上的 panic 点全是 `expect` / `assert!`，**无 abort** → `catch_unwind` 有效 |
| `bot-turn-restart` 漏发 | 前端没清场 → 用户看到两段思考 | A-6 锚点钉住「重发前必发」；前端侧 B-3 钉住清场 |
| 工具折叠行清空误伤历史轮 | 清 `rec["tools"]` 时误清历史轮工具 | `turn_record` 的 `rec` 是**本轮**记录（`begin` 清场），只含本轮；历史轮在 DB 行里 |
| MSVC 运行库版本口径 | 随包发的 dll 版本与目标机已装的冲突 | T-4=A 时以「同目录优先」分发；DLL 搜索顺序里 exe 同目录优先于系统目录 |
| 批 2 文案提到「🔁 重试」但批 4 未落 | 用户点了没按钮 | 批 2 先按「暂不提按钮」文案落；批 4 一次性回填（§3.2 已注明） |
| `LoadLibraryExW` 自检与 ort 解析规则不一致 | 自检说能加载、ort 说不能（路径解析分叉） | 自检**复刻** `lib.rs:126-135` 的解析规则（§3.5 第 1 道），并在锚点里钉住同源 |

---

## 十、实机复验

**前置**：批 1~4 完成后重新出 Windows 包（按 `AGENTS.md`「不主动出 Windows 包」——由用户明确要求时再出）。

### 10.1 复验卡

| 卡 | 步骤 | 判据 |
|---|---|---|
| W-1 | 制造断流（目标机改走会截断流的代理，或临时用 mock 网关） | `error.log` 出现 `llm.stream_failed`，且 **`err_chain` 字段含 hyper 原文**（不再是孤零零一句）；`bytes` / `elapsed_ms` 有值 |
| W-2 | 同上，看用户气泡 | 文案为「网络连接中断，本轮未完成……」，**不含**「内部错误」、**不含** `error decoding response body` |
| W-3 | 只吐思考就断流 | 后端重发一次（`llm.stream_retry` 审计出现），前端**只看到一段**思考（`bot-turn-restart` 生效） |
| W-4 | 已吐正文后断流 | 不重发；气泡保留已出正文；下方有「🔁 重试」按钮；点它能重新发起一轮 |
| W-5 | 全新机器（无 VC++ 运行库）启动 | **无** `Failed to load ONNX Runtime dylib` panic、**无** `Mutex poisoned`；审计有 `memory.embed_engine_unavailable` 且错误码为 **126**；聊天正常 |
| W-5b | 同 W-5，另查目标机 `echo %ORT_DYLIB_PATH%` | 为空 → §1.5 的「dll 在、依赖缺」结论成立；非空 → 结论需按该路径重核 |
| W-6 | 同 W-5 | 用户看到一次性提示「语义记忆不可用（缺 VC++ 2015-2022 x64 运行库），请安装 …」 |
| W-7 | macOS 回归 | `cargo nextest` 全绿；记忆检索仍走语义路径（`memory.embed_engine_unavailable` 不出现） |

### 10.2 结果回填位

| 卡 | 结果 | 备注 |
|---|---|---|
| W-1 | ☐ | |
| W-2 | ☐ | |
| W-3 | ☐ | |
| W-4 | ☐ | |
| W-5 | ☐ | |
| W-5b | ☐ | |
| W-6 | ☐ | |
| W-7 | ☐ | |
