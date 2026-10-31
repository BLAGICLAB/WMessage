# 专用 HTTP 工具与目的地机制（2026-10-30）

> 本文是**改造方案**（未落地）。它承接《`docs/RUN-PYTHON-NETWORK-POLICY-2026-10-30.md`》
> 的能力前置项 C1–C4（C4 已拍板：新起 `http_request`，`fetch_url` 不动），
> 落地后才轮到那份方案的口径层（「`run_python` 不联网」）。
> 落地后的最终状态以 `SPEC.md` 与代码为准；实施完成后在文末「实施结果」回填。
>
> **不做这一步的后果**：`run_python` 断网是能力回退，而回退要靠这里补回来。
> 先断网再补，中间是真空期——用户此刻调 API、下载数据文件是真的做不了。

## 一、要解决的问题

`run_python` 断网后，两类真实需求（2026-10-30 用户拍板）必须有替代：

| 需求 | 现状为什么不行 | 要补的 |
|---|---|---|
| 调外部 API 办事（POST / JSON / 鉴权头） | `fetch_url` 是 GET-only，直连拒 `application/json`（`src-tauri/src/bot_web/fetch.rs:299-307`）；唯一兜底是 Jina Reader 第三方代理（`fetch.rs:181-200`），慢、限流、且目标 URL 发给第三方 | C1：通用 HTTP 请求能力 |
| 下载数据文件给脚本处理 | `fetch_url` 是网页正文提取器（html2text + 主块提取），CSV 过了类型检查也会被抽成碎文本 | C2：下载原样落产物目录 |

外加一项**不可省**的配套：

| 项 | 内容 |
|---|---|
| C3 目的地机制 | 模型选定的目的地不能是任意公网——用户配置的域名白名单默认放行，白名单外每次弹窗授权、**仅本次生效** |

## 二、工具形态（C4 已拍板：新起 `http_request`，`fetch_url` 不动）

### 2.1 为什么不扩 `fetch_url`

`fetch_url` 的现有语义是「读网页正文」：公网闸 → 逐跳重定向校验 → 内容类型闸 → 主块提取
→ html2text → 30K 字符切片 + offset 续读（`bot_web/fetch.rs` 全文 + `bot/tools/doc.rs:671`）。
POST 语义与二进制下载塞不进去；硬扩会让它从「读正文工具」变成「什么都干工具」，而它的
schema 描述、主块提取逻辑、续读语义都是围绕读正文写的。

`http_request` 与 `fetch_url` 的分工：

- `fetch_url` —— 读公开网页正文（现有行为，语义不动）
- `http_request` —— 通用 HTTP 客户端：方法 / 请求体 / 自定义头 / 下载落盘

### 2.2 schema

```json
{
  "type": "function",
  "function": {
    "name": "http_request",
    "description": "发一个 HTTP 请求（GET/POST/PUT/PATCH/DELETE/HEAD），返回状态码、响应头与正文（超限截断）；要落文件时给 saveAs，文件存进 AI_Gen_Files 并返回绝对路径，便于交给 run_python 本地处理。只允许 http/https 公网地址（环回/私网/链路本地一律拒）。目的地默认走用户配置的白名单，白名单外的每次都要弹窗问用户，本次批准只对这一次有效——所以优先请求用户已经配过的域名；用户会看到弹窗时先说明为什么需要访问它。请求头里带的凭据（Authorization 等）不会回显、不会写进审计日志。读网页正文用 fetch_url；调 API / POST JSON / 带鉴权头 / 下载数据文件才用它。",
    "parameters": {
      "type": "object",
      "properties": {
        "url": { "type": "string", "description": "目标 URL（http/https 公网地址）" },
        "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"], "default": "GET", "description": "HTTP 方法，默认 GET" },
        "headers": { "type": "object", "description": "请求头（字符串键值对），用于鉴权等；Authorization 等敏感头的值既不回显也不进审计日志" },
        "body": { "type": "string", "description": "请求体（配合 bodyType）" },
        "bodyType": { "type": "string", "enum": ["text", "json", "form"], "default": "text", "description": "请求体类型：text 原样 / json 按 JSON 序列化 / form 按 x-www-form-urlencoded 编码" },
        "saveAs": { "type": "string", "description": "可选：把响应体原样存成文件（落 AI_Gen_Files），返回文件绝对路径与大小，不返回正文。用于下载 CSV / 数据集等文件再交给 run_python 本地处理" },
        "timeoutSecs": { "type": "integer", "description": "可选：请求超时秒数，默认 30，上限 120" },
        "maxBytes": { "type": "integer", "description": "可选：响应体字节上限，默认 10MB，上限 50MB" }
      },
      "required": ["url"]
    }
  }
}
```

### 2.3 返回

- 未给 `saveAs`：状态码 + 响应头（脱敏后）+ 正文（超 `maxBytes` 截断并标注，可按 `offset` 续读，形态对齐 `fetch_url` 的 30K 切片）
- 给了 `saveAs`：状态码 + `content-type` + 保存路径 + 字节数（**不回显正文**）
- 请求头**只回显名字不回显值**（见 §四凭据处理）

## 三、目的地机制（C3）

### 3.1 两类目的地，分开判

| 类 | 例子 | 判定 |
|---|---|---|
| **产品自有出口** | 模型 provider、搜索引擎 API（按配置）、`r.jina.ai` 渲染代理、`install_skill` 的固定域名 | 产品自己定的出口，**不经用户闸**——它是产品的基础设施，用户配 provider 时已经知情 |
| **用户目的地** | 模型在任务里选定的 URL（`fetch_url` / `http_request`） | 走用户闸（下节） |

分层的理由：产品自有出口是「用户装这个应用时就同意的依赖」，再弹一次窗是噪声；而用户目的地
是模型**当场选**的，可能来自不可信内容（网页、微信消息），所以必须用户点头。

### 3.2 用户闸的判定顺序

```
1. host 命中 cfg.allowed_domains（精确匹配或 *.suffix 匹配）  → 放行（用户主动信任）
2. 否则按 perm_mode 三分支（与 bot_fs 的文件白名单完全同一套语义）：
   strict → 拒绝
   ask    → 弹窗请求确认；批准仅本次生效，不落盘、不泛化成「允许联网」
   yolo   → 放行 + 写审计
3. 弹窗 60s 无响应 → 默认拒绝（沿用 bot_slash::ask_user_confirm 的安全兜底）
```

`strict` / `ask` / `yolo` 三档是既有的授权模式（`bot/config/types.rs:125-128`），文件工具已经
用它做三分支。目的地闸复用同一套，**不新增第四种授权形态**。

「仅本次生效」是硬要求：不做「始终允许该域名」按钮——持久信任由设置页白名单承担。想要
长期访问某个域名，用户自己把它加进白名单。这样授权面不会积累成第二套白名单。

### 3.3 白名单配置

- `BotConfig.allowed_domains: Vec<String>`（`bot/config/types.rs`，camelCase `allowedDomains`，
  `#[serde(default)]`，空 = 无白名单 → 全部走弹窗）
- `BotConfigView.allowed_domains: Vec<String>`（同文件 `:404` 附近的 view 结构，前端透传）
- 设置页新增一个输入域，每行一个 host；支持 `*.example.com` 前缀匹配

### 3.4 待拍板 D1：`fetch_url` 是否也纳入用户闸

**推荐：纳入。** 理由是不自洽——`fetch_url` 是 GET，但 GET 一样能外传（把秘密拼进查询串
即可）。若 `fetch_url` 留在闸外，`http_request` 的目的地限制就是个筛子：模型想带数据出去
时用 `fetch_url` 就绕过去了，闸形同虚设。

代价是「读一个新网站多一次弹窗」。缓解：用户把常用域名加进白名单；弹窗文案直接给出
host 与「本次有效」。

**若选乙（只管 `http_request`）**：体验好，但必须在 SPEC 里写明「`fetch_url` 仍可读任意
公网，GET 查询串外传不在目的地闸的覆盖范围内」——这是明知的缺口，不是忽略。

### 3.5 无人值守场景的已知后果

定时作业（`bot_scheduler`，每 30s 扫到点任务自动执行，无人在场）里若模型请求白名单外的
目的地 → 弹窗没人点 → 60s 超时 → 拒绝 → 任务失败。

这是设计内的正确失败（fail-closed），不是 bug。缓解写在提示词里：定时作业只用白名单内的
目的地。若这条不可接受，唯一出路是给定时作业单独的白名单，而不是放开弹窗。

## 四、凭据处理（硬要求，不是可选项）

`http_request` 能带 `Authorization` 头，模型也可能把 token 写在 `body` 里。处理规则：

1. **不回显**：响应里只出现请求头的**名字**，不出现值。
2. **不进审计**：审计事件只记 `host` / `method` / `status` / `bytes` / `dest` / `saved`；
   不记 header 值、不记 body、**不记完整 URL**（查询串可能带 token——只记 host）。
3. **不进产物**：`saveAs` 落盘的是响应体，请求头不进文件。
4. **不进提示词回灌**：工具返回值会进模型上下文，所以不回显是唯一能挡住凭据进上下文的
   手段。
5. **建议用户把凭据放 keyring / 环境变量**（仓里既有的做法），别写在对话里——写进对话就
   已经进了 LLM 上下文，本工具管不了。

## 五、代码落点

| 新增 | 位置 | 职责 |
|---|---|---|
| `bot_web/destination.rs` | 新文件 | 目的地分类：产品自有出口清单、白名单匹配（精确 / `*.suffix`）、perm_mode 三分支判定 |
| `bot_web/http.rs` | 新文件 | 请求执行：复用 `check_public_url`（`bot_web/fetch.rs:34`）→ 解析出的公网地址钉给 reqwest（`http_client_pinned`，堵 DNS 重绑定）→ 不跟随自动重定向、手动逐跳校验 Location（沿用 `fetch.rs:216-260` 的做法）→ 流式封顶（沿用 `read_body_capped`，不信 Content-Length） |
| `bot/tools/http.rs` | 新文件 | `tool_http_request`：参数解析 → 目的地判定 → 授权 → 执行 → 结果整形。放新文件而非 `bot/tools/doc.rs`（已 835 行） |
| `SCHEMA_HTTP_REQUEST` | `bot/registry/schemas.rs` | schema 常量 + `TOOLS_TABLE` 条目 |
| `call_http_request` | `bot/registry/handlers.rs`（`:216` 旁边） | handler，照 `call_fetch_url` 的形 |
| `allowed_domains` | `bot/config/types.rs` | 配置字段 + view 字段 |
| 设置页白名单编辑 | `src/components/SettingsPage/` | 域名列表编辑 |
| `mod destination; mod http;` | `bot_web.rs`（门面） | `bot_web.rs` 是门面文件，子模块在此声明（现为 `mod fetch; mod html; mod search;`） |

复用既有件（不重造）：

| 件 | 位置 | 可见性 | 用途 |
|---|---|---|---|
| `check_public_url` | `bot_web/fetch.rs:34` | `pub(crate)` | 公网闸：协议 + 主机 + DNS 解析出的所有 IP 都要公网 |
| `http_client_pinned` | `bot_web.rs:118` | `pub(crate)` | 钉住解析结果的 client，堵 DNS TOCTOU（校验与请求之间不再二次解析） |
| `http_client_builder` | `bot_web.rs:106` | 私有（门面内） | connect 15s / 总 30s / 禁自动重定向；`http_request` 要自建 builder 才能按 `timeoutSecs` 调超时 |
| `read_body_capped` | `bot_web.rs:131` | 私有（门面内） | 流式封顶，不信 Content-Length |
| `ask_user_confirm` | `bot/bot_slash.rs:420` | `pub` | 确认弹窗，60s 无响应默认拒 |
| `db::gen_dir` | `db/paths.rs:31` | `pub` | AI_Gen_Files 目录 |
| `bot_artifacts::register` | `bot/bot_artifacts.rs:33` | `pub` | 产物登记 |
| `audit_event!` | `src-tauri/src/audit.rs:703` | 宏 | 审计留痕 |

**不要复用 `bot_web::search::http_client()`**（`search.rs:61`）：它是给搜索引擎用的全局复用
client，**不钉 IP**，超时口径也不同（降级路径 60s）。`http_request` 的目标地址由模型指定、
每次都不同，必须走 `http_client_pinned` 这条路——否则 DNS TOCTOU 的窗口又开了。

**容量线**：`FETCH_MAX_BYTES = 10MB`（`bot_web.rs:23`）是页面抓取的传输闸；`http_request`
沿用同一量级作默认，`maxBytes` 上限 50MB——两者的用途不同，别混成一条线。

## 六、审计事件

事件名 `bot.http_request`，字段：

| 字段 | 说明 |
|---|---|
| `host` | 目标主机（**不记完整 URL**） |
| `method` | HTTP 方法 |
| `status` | 响应状态码 |
| `bytes` | 响应体字节数 |
| `dest` | `product` / `whitelist` / `asked` / `yolo` / `denied` |
| `saved` | 是否落盘（落盘时另记产物路径，不记内容） |

拒绝与授权也要记：`dest=denied`（strict 拒）、`dest=asked` + 授权结果（批准 / 超时拒绝）。
凭据、body、查询串一律不记。

## 七、批次切分（一个逻辑变更一个 commit）

| 批 | commit 主题 | 内容 |
|---|---|---|
| 1 | `feat(bot): 目的地判定与白名单配置（配置字段 + 白名单匹配 + perm_mode 三分支）` | `bot_web/destination.rs`、`bot/config/types.rs`、设置页白名单编辑 |
| 2 | `feat(bot): 新增 http_request 工具（方法 / 请求体 / 自定义头）` | `bot_web/http.rs`、`bot/tools/http.rs`、schema + handler + TOOLS_TABLE |
| 3 | `feat(bot): http_request 支持下载到产物目录` | `saveAs` 落地 + 产物登记 + 路径校验 |
| 4 | `feat(bot): fetch_url 接入目的地闸`（D1 定为「纳入」才做） | `bot/tools/doc.rs` + `bot_web/fetch.rs` |
| 5 | `docs(agent): 提示词告知 http_request 的存在与用法` | `prompts/system.rs` 联网工具规则段 + 子 agent profile |

批 2 之后 `http_request` 已可用，但模型不知道它存在（批 5 才写进提示词）——这两批之间
工具是「在但没人用」的状态，可接受；批 5 必须与口径层的「不联网」批同批落地，否则模型
被要求不联网却不知道替代品在哪。

## 八、锚点测试

| # | 位置 | 断言 |
|---|---|---|
| 1 | `bot_web/destination.rs` | 白名单命中（精确 / `*.suffix`）→ 放行；未命中 + `strict` → 拒；未命中 + `ask` → 走确认 |
| 2 | `bot_web/destination.rs` | 产品自有出口不经用户闸 |
| 3 | `bot/tools/http.rs` | 响应里不含 `headers` 的**值**（构造 `Authorization: Bearer secret`，断言输出不含 `secret`） |
| 4 | `bot/tools/http.rs` | 审计 kv 不含 header 值与 body |
| 5 | `bot/tools/http.rs` | `saveAs` 拒绝含路径分隔符 / `..` 的名字（防目录穿越） |
| 6 | `bot_web/http.rs` | 公网闸复用：环回 / 私网 / 链路本地（含 `169.254.169.254`）被拒；请求走 `http_client_pinned`（钉 IP，不二次解析）——沿用 `bot_web/tests.rs` 的既有用例形态 |
| 7 | `bot/registry/registry_tests.rs` | `http_request` 已登记进 `TOOLS_TABLE`（schema 与闸必须算出同一份工具集，既有守卫覆盖） |

## 九、风险

| 风险 | 概率 | 缓解 |
|---|---|---|
| 审批疲劳（白名单外每请求一次弹窗） | 高（若 D1 定为纳入） | 白名单默认放行；弹窗只问一次不重复；持久信任走设置页；定时作业改用白名单内目的地 |
| 白名单写太宽（`*.example.com`） | 中 | schema 与设置页都提示这是域名级信任；SPEC 留档 domain fronting 提醒（只看域名不看 TLS 内容的白名单单独用等于没闸） |
| 定时作业撞上白名单外目的地 → 无人值守失败 | 中 | 提示词里写明定时作业只用白名单内目的地；这是 fail-closed 的正确失败，不放宽弹窗 |
| `http_request` 变成外传通道（读到的文件内容 POST 出去） | 中 | 目的地闸是唯一防线 → 白名单为空时全靠弹窗；凭据类请求头不回显、不进审计 |
| 模型不用 `http_request`，仍去 `fetch_url` 绕 | 中（仅当 D1 选乙） | 选甲则封住；若选乙，必须在 SPEC 明写这是已知缺口 |
| 响应体撑爆内存 | 低 | 流式封顶（`read_body_capped`），不信 Content-Length；`maxBytes` 上限 50MB |

## 十、验收口径

1. `cd src-tauri && cargo test --lib` —— 七个新锚点测试 + 既有 `bot_web` / `bot` / `registry`
   测试全绿。
2. `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` —— 无警告。
3. `bash scripts/test-fast.sh` —— 快门禁过。
4. 口径对拍：
   ```bash
   grep -n "allowed_domains" src-tauri/src/bot/config/types.rs
   grep -n "http_request" src-tauri/src/bot/registry/schemas.rs
   grep -n "mod destination;\|mod http;" src-tauri/src/bot_web.rs
   grep -rn "Authorization" src-tauri/src/bot/tools/http.rs   # 只能出现在「不回显」相关代码里
   # 不得直接用搜索那个全局 client（不钉 IP）
   grep -rn "search::http_client" src-tauri/src/bot_web/http.rs   # 应无输出
   ```
5. 实机抽验（需本机可出网）：
   - 白名单内的域名 → 直接放行，无弹窗；
   - 白名单外的域名 → 弹窗出现、批准后仅本次生效、再来一次要重新弹；
   - `Authorization` 头的请求 → 工具返回值与审计日志里都搜不到该 header 的值；
   - 下载一个 CSV → 落 `AI_Gen_Files` → `run_python` 读它做统计并画图（**验证断网后能力不回退
     的关键一步**）；
   - 请求 `http://127.0.0.1:PORT` → 被公网闸拒。

## 十一、实施结果（2026-10-31 回填）

- 实际落地：已由《`docs/RUN-PYTHON-SECURITY-REFIT-PLAN-2026-10-31.md`》整合实施
  （批 1+2 合并 cf57b52b、批 3 73bdfd62）。§4.1 回灌切片按整合方案定死 30K；
  §4.2 每跳判落地；§4.3 三小件 + `keyring:`/`env:` 凭据引用、会话粒度授权（B5）、
  无人值守快速拒（§5.4）、子 agent strict（§5.5）、MCP 并入判定源（§5.6）、
  punycode 归一（§5.7）全部落地。
- 与方案的偏离登记：见整合方案 §十二（批合并 / saveAs 不进登记面 / preview 特例落点）。
- D1（`fetch_url` 是否纳入目的地闸）最终拍板：**不做**（2026-10-31）——SPEC 记已知
  缺口 + 提示词查询串防线兜底。
- 实机抽验：待做（ACC-P-11 卡）。
