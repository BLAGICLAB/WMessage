# run_python 联网口径：改为「不联网」，联网能力收进专用工具路径（2026-10-30）

> 本文是**改造方案**（未落地）。落地后的最终状态以 `SPEC.md` 与代码为准；
> 实施完成后在文末「实施结果」回填实际落地情况与偏离登记。
>
> **方向修订记录（2026-10-30）**：本文初稿主张「能力层写『可联网』+ 联网优先
> `web_search`/`fetch_url`」。经必要性论证与用户拍板，方向改为
> **`run_python` 不联网，联网能力收进专用工具路径**。初稿的三处硬伤见 §一，
> 论证见 §二，拍板见 §三。外部依据见
> 《`docs/AGENT-SANDBOX-NETWORK-RESEARCH-2026-10-30.md`》。

## 一、初稿为什么被推翻

初稿主张：能力层写「可联网」、使用策略层写「联网优先
`web_search`/`fetch_url`」。三处硬伤：

### 1.1 「优先」是弱约束，初稿自己还开了口子

初稿的提示词草案写「run_python 里直接联网只用于这两个工具做不了的事（自定义协议、
**批量请求**、特殊鉴权）」。模型自证「这活属于批量」，许可就到手了——而批量恰恰是
`fetch_url` 最难用、模型最想绕的场景。「优先」在这种场景是装饰。

### 1.2 schema 里的「可联网」是许可证，不是披露

工具描述每一轮都被重读，模型把能力词读成许可；而系统提示词的「不得」是整条栈里
权威最高的层。初稿把最强的层削弱、把最弱的层加强，方向反了。

### 1.3 失败模式不对称，而且有无人值守路径

禁网 → 任务做不成，消息里看得见、可恢复；开网 → 脚本读文件再 POST 出去，**脚本正文
可见但载荷不可见**，静默、不可恢复。而定时作业是无人值守的：`bot_scheduler` 每 30s
扫一次到点任务并自动执行，注释明写「绕开 exec_steps 逐步确认（**无人在场**，直接整体
执行）」（`src-tauri/src/bot_scheduler.rs:1-9`）。调研里那三层绕过链与注入案例，全部
发生在提示词层失守的时候。

### 1.4 初稿只看了主聊天提示词，漏了子 agent（后补）

见 §二.2——这一条最后成了本次方向最硬的依据。

## 二、必要性论证

### 2.1 两个问题要拆开：bot 需要联网 ≠ run_python 需要联网

必要性论证的正确形式是枚举需求，不是表态。bot 显然需要联网（时效性判断、来源标注、
调外部服务）。要问的是：**有多少需求是「只有脚本能上网」才能满足的？**

产品**已经**有的联网路径，全部受控：模型 API（用户配的 provider）、MCP 服务器（用户配的
server）、`install_skill`（GitHub / SkillHub / zip 直链，固定域名）、`web_search`、
`fetch_url`。OCR 是纯本地（macOS Vision / Windows PP-OCRv6，`src-tauri/src/ocr.rs:3-5`）。

| 需求 | 现在能走哪条路 | 只有脚本能上网吗 |
|---|---|---|
| A. 知道最新信息、读网页 | `web_search` + `fetch_url`（带公网闸 + 来源标注） | 否 |
| B. 调外部 API 办事（POST / JSON / 鉴权头） | **不能**——`fetch_url` 是 GET-only，且直连拒 `application/json`（`src-tauri/src/bot_web/fetch.rs:299-307`） | 否——专用工具扩展后接管 |
| C. 下载数据文件给脚本处理 | **不能**——`fetch_url` 是网页正文提取器（html2text + 主块提取），CSV 过了类型检查也会被抽成碎文本 | 否——「下载到产物目录」能力接管 |
| D. 研究中跟进新发现的任意端点 | 不能——目的地事先未知 | **是**（唯一的残留，见 §八） |

用户 2026-10-30 的真实用例是 **B 与 C**（调外部 API 办事；下载数据文件再处理），
**不含 D**。B 与 C 都能落在专用工具路径上，D 不构成必要性——于是 `run_python` 联网的
独占必要性趋近于零。

### 2.2 子 agent 的三段式分工里，联网本来就只属于 recon

这一条是本次方向最硬的依据，且来自本仓设计而非外部研究。

子 agent 工具白名单（`src-tauri/src/bot/registry/schemas.rs:1010-1048`）：

| 档 | 联网工具 | `run_python` |
|---|---|---|
| `RESEARCH_TOOLS` | `web_search` + `fetch_url` | 无 |
| `CODER_TOOLS` | 无 | 有 |
| `GENERAL_TOOLS` | `web_search` + `fetch_url` | 有 |

角色 profile（`src-tauri/src/prompts/subagent.rs:155-177`）写明三段式分工：recon 联网
调研产出报告 → executor「**优先读 recon 报告路径作为输入，不重新调研**」→ verifier 只读
验收、禁用一切写类与联网工具。

**设计意图是：联网属于 recon 阶段，执行与验收阶段不联网。** `run_python` 的直连能力在
coder / executor 子 agent 里开的，正是一条绕过三段式分工的暗道——设计里「不联网」的执行
阶段，靠这条暗道实际能联网。

于是本次决策的定性变了：**断网不是砍能力，是把一条暗道堵上，让执行阶段回到设计意图。**

附带发现（初稿漏掉的硬要求）：主聊天提示词的安全红线（`src-tauri/src/prompts/system.rs:74`）
**只作用于主聊天**。子 agent 的 profile 提示里对「run_python 不得联网」一字未提
（`prompts/subagent.rs:129-143` 的 coder / general 段只列工具与用法）。所以任何联网口径
必须同时落在**主聊天提示词 + 子 agent profile + 工具描述**三处，否则又是一条有洞的口径。

### 2.3 内部一致性：OCR 的隐私红线与脚本联网不能同时成立

OCR 模块头写着「隐私红线（硬约束）：图片字节只在内存里交给本地引擎，本模块没有任何
网络调用，识别过程**绝不上传外网**；网络路径（http/https/data:/file:）在入口处直接拒」
（`src-tauri/src/ocr.rs:3-5`，schema 里重复一遍：`registry/schemas.rs:66`）。OCR 之所以能
写下这条红线，前提是**它没有出网能力**。

而 `run_python` 没有文件隔离——它能读任何文件，包括图片、密钥、账本。**「run_python 可
联网」与「本地内容绝不出网」这两条产品姿态，不能同时成立。** 模型读到一个文件 → POST
出去，这条通道一旦开着，OCR 那条红线就是纸面的。

这不是外部研究说「沙箱不安全」，是产品自己的两条原则打架。

## 三、决策（2026-10-30 拍板）

1. **`run_python` 不联网。** 主聊天提示词、子 agent profile、工具描述三处统一写明：
   脚本不走网络，联网走 `web_search` / `fetch_url`。
2. **bot 需要联网；联网能力收进专用工具路径。** 专用工具要补齐两类能力（§五.5）：调外部
   API（POST / JSON / 鉴权头）、下载数据文件到产物目录。方案见
   《`docs/HTTP-REQUEST-TOOL-DESIGN-2026-10-30.md`》。
3. **目的地限制**：用户配置的域名白名单默认放行；白名单外每次弹窗授权，**仅本次生效**
   （形态参照仓里既有的「读/写路径白名单 + 白名单外逐次确认」模式）。
4. **apps 程序契约的「禁联网」改成交付约定**：不再像能力禁令，改成「脚本要自包含、
   不依赖外网」（它与 run_python 同一 py 沙箱，本来就拦不住）。
5. **超时口径统一到 120s**：两处 schema 文案 + SPEC + 两处代码注释 + 测试名，共七处对齐
   （清单见 §五.2）。
6. **技术闸（macOS seatbelt 断网）另立项目，本次不做。** 方向已定，软约束够用；真要
   把「不联网」变成硬边界是独立工程（Windows 无廉价对等物，见 §八）。

## 四、口径定稿：三层各说各的

| 层 | 口径 | 落点 |
|---|---|---|
| **能力层** | 专用联网工具**能**联网（公网闸 + 目的地白名单 + 按次授权 + 来源标注）；`run_python` **不**联网 | SPEC + 设置页文案 + 工具描述 |
| **使用策略层** | 联网任务一律走 `web_search` / `fetch_url` / `http_request`；`run_python` 只做本地计算 | 主聊天提示词 + 子 agent profile |
| **程序交付层** | 上架程序脚本自包含、不依赖外网 | apps 程序契约 |

三层之间必须写进提示词的因果：**专用工具路径上多三道保护**——`bot_web` 的公网闸（挡
环回/私网/链路本地，含云元数据端点 `169.254.169.254`，`bot_web/fetch.rs:402-404`）、结果
附来源（`prompts/system.rs:62`）、目的地白名单 + 按次授权。脚本直连这三道都没有。理由
只写「优先」不够，得写「哪个带保护」——模型才有遵守它的动机。

## 五、改动清单

### 5.1 口径层：三处覆盖（硬要求）

**① 主聊天提示词 `src-tauri/src/prompts/system.rs:74`（安全红线第一条）**

改前：

```
- 你只有白名单工具可用；run_python 是唯一执行代码的工具，它以当前用户权限运行、无文件系统隔离，不得用它读写白名单外的路径、起子进程或访问网络，绝不修改系统设置或访问系统目录；
```

改后（同段，网络禁令保留，补上路由与理由）：

```
- 你只有白名单工具可用；run_python 是唯一执行代码的工具，它以当前用户权限运行、无文件系统隔离；不得用它读写白名单外的路径或起子进程，不让它访问网络（联网一律走 web_search / fetch_url / http_request——它们只走允许的目的地、结果带来源，脚本直连这三道保护都没有），绝不修改系统设置或访问系统目录；
```

约束核对：保留「不得读写白名单外的路径」「不得起子进程」「绝不修改系统设置或访问系统
目录」；保留「绝不」二字（`prompts/mod.rs:102` 锚点测试依赖它）。本次只把网络那一项从
并列禁令改成「禁令 + 路由 + 理由」。

**② 子 agent profile `src-tauri/src/prompts/subagent.rs`**

`PROFILE_CODER`（`:129-137`）与 `PROFILE_EXECUTOR`（`:163-169`）各补一句：

```
- run_python 不联网（它直连外网没有任何保护）；需要查资料由上游 recon 阶段负责，或用 fetch_url / web_search。
```

`PROFILE_GENERAL`（`:139-143`）同时有 `run_python` 与 `web_search` / `fetch_url`，补同一句。

`PROFILE_RECON`（`:155-160`）与 `PROFILE_VERIFIER`（`:172-177`）本就没有 `run_python`
（recon 明令禁写类工具，verifier 明令禁一切写类与联网工具），**不动**。

注意：新增句子会进 `prompts/mod.rs` 的「全部 prompt 单一清单」通用断言与
`subagent.rs:232-248` 的「profile 提示必须点名白名单里每个工具」测试——新增词必须落在
既有白名单内，否则测试红。

**③ 工具描述 `src-tauri/src/bot/registry/schemas.rs:139`（`SCHEMA_RUN_PYTHON`）**

改前：「……独立临时目录，无文件系统隔离；默认超时 60s。……」
改后：「……独立临时目录，无文件系统隔离；**不联网**（联网走 web_search / fetch_url /
http_request）；默认超时 120s，上限 600s。……」

schema 只陈述**能力边界**（不联网）+ 路由，不写「为什么」，理由写在提示词一处，避免同一
件事两处措辞各自漂移。

**④ apps 契约 `src-tauri/src/bot/registry/schemas.rs:264`（`SCHEMA_APPS_CREATE`）**

改前：「……summary 控制在 300 字内；**禁联网**；敏感数据（身份证号等）放 data/ 配置文件……」

改后：「……summary 控制在 300 字内；**脚本要自包含、不依赖外网**（运行环境不保证外网可达，
需要联网取数的活不适合上架成程序）；敏感数据（身份证号等）放 data/ 配置文件……」

**⑤ apps 契约 `src-tauri/src/bot/registry/schemas.rs:265`（`SCHEMA_APPS_UPDATE_FILES`）**

改前：「脚本契约同 apps_create（开头校验段 + ##RESULT## 哨兵收尾 + argv[1] 解析目录 +
产物写 argv[2] + **禁联网**）」

改后：「脚本契约同 apps_create（开头校验段 + ##RESULT## 哨兵收尾 + argv[1] 解析目录 +
产物写 argv[2] + **脚本自包含不依赖外网**）」

### 5.2 超时口径（120s）

| # | 文件:行 | 改动 |
|---|---|---|
| 6 | `src-tauri/src/bot/registry/schemas.rs:139` | 「默认超时 60s」→「默认超时 120s，上限 600s」 |
| 7 | `src-tauri/src/bot/registry/schemas.rs:156` | `SCHEMA_AST_INSPECT` 描述「默认 60s 超时」→「默认 120s」（同一沙箱、同一常量；这是模型直接读的文案，漏了它等于漏了一半） |
| 8 | `SPEC.md:294` | 「+ 60s 超时」→「+ 120s 超时」 |
| 9 | `src-tauri/src/py/ast.rs:912` | 注释「`run_python` 的 60s 超时」→「120s 超时」 |
| 10 | `src-tauri/src/bot/services/tests/integration.rs:292` | 注释「远超 run_python 默认 60s 超时对应的 70s CPU 限额」→「120s」 |
| 11 | `src-tauri/src/bot_py/tests.rs:215` | 节注释「默认 60s」→「默认 120s」 |
| 12 | `src-tauri/src/bot_py/tests.rs:218` | 测试名 `resolve_timeout_default_60s` → `resolve_timeout_default_120s`（断言用常量，行为不变，只去掉名字漂移） |

真值：`src-tauri/src/bot/constants.rs:94` `PYTHON_TIMEOUT_SECS = 120`；上限
`src-tauri/src/py/runtime.rs:41` `MAX_TIMEOUT_SECS = 600`。

漂移点一共 7 处（本次核出的全量）：`schemas.rs:139`、`schemas.rs:156`、`SPEC.md:294`、
`py/ast.rs:912`、`services/tests/integration.rs:292`、`bot_py/tests.rs:215`、`:218`。前两处
是模型直接吃进去的文案，优先级最高。

> 核查口径：`src-tauri/src/.mimosa/` 是钩子基线快照（已 gitignore），里面留着旧文案属
> 正常，不要跟着改；grep 时要排除，否则判据永远不干净。

### 5.3 文档记账

| # | 文件 | 改动 |
|---|---|---|
| 13 | `SPEC.md:136` | run_python 段补「不联网（联网走 web_search / fetch_url）」及理由；补「沙箱无网络隔离」这一事实 |
| 14 | `SPEC.md`（新增条目） | 2026-10-30 口径变更条目：为什么从「提示词禁网」改为「三处统一的不联网口径 + 联网能力收进专用工具」，以及「技术层仍能联网」这一已知差异 |
| 15 | `DEVLOG.md`（追加） | 只记「为什么换方向」：初稿为何被推翻（弱约束 + 许可证化 + 失败模式不对称）、子 agent 三段式分工这条硬依据、为什么不建网络闸 |
| 16 | `AGENTS.md` 地图 | 本文件一行 + 调研文档一行 + 能力前置方案一行 |

### 5.4 核对后不动的地方（记一笔，免得下次又查）

- `src/components/SettingsPage/BotApiSection.tsx:61` —— 现写「Python 编程免开关直接执行
  （以本机用户权限，**可联网**）」。**这次要改**：与新口径矛盾，且那句把沙箱能力当成了
  yolo 模式的既成事实陈述，用户从没选过。改为「不联网」。
- `src/components/SettingsPage/BotCoreSection.tsx:195` —— 已写「默认超时 120s、上限
  600s」，不动。
- `DEVLOG.md:1122-1125`（2026-10-20 条目里的「禁联网」）—— 历史记录，不改。
- `src-tauri/src/bot_web` 的公网闸实现 —— 既有安全边界不动，只在能力前置里扩展目的地
  判定。
- `docs/` 下的设计文档 —— 全仓检索无一处写「run_python 不能联网」，不动。

### 5.5 能力前置（依赖项：§五.1 的口径层不能单独落）

断网是能力回退，回退由下面两项补回来。两项都在**专用工具路径**上，因此 §四的三道保护
（公网闸 / 来源标注 / 目的地限制）继续成立。

| 能力 | 现状 | 需要补的 | 对应需求 |
|---|---|---|---|
| **C1 调外部 API** | `fetch_url` GET-only，直连拒 `application/json`（`bot_web/fetch.rs:299-307`）；仅 Jina Reader 第三方代理兜底能救（`fetch.rs:181-200`，慢、限流、且目标 URL 发给第三方） | 方法（GET/POST/PUT/DELETE…）+ 请求体（JSON / 表单）+ 自定义请求头（承载鉴权）+ 响应体按类型返回 | B |
| **C2 下载数据文件** | `fetch_url` 是网页正文提取器，CSV 会被抽成碎文本 | 把二进制/CSV 原样下载到 `AI_Gen_Files` 并返回路径，供 `run_python` 本地读文件处理 | C |

**C3 目的地机制**（配套，不是可选项）：新增「允许访问的域名」白名单（设置页维护、配置
存储）；白名单内默认放行；白名单外每次弹窗授权、**仅本次生效**、不得做成「永久允许」。
形态参照仓里既有的「读/写路径白名单 + 白名单外逐次确认」。每次放行与授权要留审计事件。

**C4 工具形态：已拍板（2026-10-30）——新起 `http_request`，`fetch_url` 不动。**
`fetch_url` 的现有语义是「读网页正文」（主块提取 + 截断续读），POST / 二进制下载塞不进去，
扩完后它就不再是「读正文」工具了。用户最初选的是「扩 `fetch_url`」，见下方偏离登记。

C1–C4 的完整方案在《`docs/HTTP-REQUEST-TOOL-DESIGN-2026-10-30.md`》（工具 schema、目的地
分层与判定顺序、凭据处理、代码落点、审计事件、批次切分、锚点测试、待拍板点 D1）。
**该方案落地前，口径层不能动**——否则「断网」就先砍了能力、后补回来，中间是真空期。

> 偏离登记（待回填）：用户 2026-10-30 在选项里选了「扩 `fetch_url` 支持 POST / JSON」，
> 后按本方案 §二 的理由改选「新起 `http_request`」。

## 六、批次切分（一个逻辑变更一个 commit）

| 批 | commit 主题 | 文件 |
|---|---|---|
| 1 | `feat(bot): 专用 HTTP 工具补 POST/JSON/下载能力 + 目的地白名单与按次授权` | C1–C4，五批切分见《`docs/HTTP-REQUEST-TOOL-DESIGN-2026-10-30.md`》§七。**这批不做完，口径层不动** |
| 2 | `docs(agent): run_python 联网口径统一为「不联网」——主聊天提示词 + 子 agent profile + 工具描述三处` | `prompts/system.rs`、`prompts/subagent.rs`、`bot/registry/schemas.rs:139` + 锚点测试 |
| 3 | `docs(agent): apps 程序契约的「禁联网」改成交付约定` | `bot/registry/schemas.rs:264/265` + 锚点测试 |
| 4 | `docs(agent): run_python 超时口径统一到 120s（两处 schema / SPEC / 两处注释 / 测试名）` | `registry/schemas.rs:139` + `:156` 超时句、`SPEC.md:294`、`py/ast.rs:912`、`bot/services/tests/integration.rs:292`、`bot_py/tests.rs:215` + `:218` |
| 5 | `docs(agent): 设置页文案与 SPEC/DEVLOG/AGENTS 记账` | `BotApiSection.tsx:61`、`SPEC.md:136` + 新条目、`DEVLOG.md`、`AGENTS.md`、本文件回填 |

批 2 与批 3、4 都动 `registry/schemas.rs:139`——按顺序做，批 2 的 schema 描述处于半成品
状态（不联网已写、超时还是旧值）。若要求每个 commit 自洽，批 2 与批 4 合并。

批 2 的提示词与工具描述要点名 `http_request`（替代品），所以批 2 必须晚于批 1——**先断网再
补能力，中间是真空期**。批 4（超时对齐）与批 3（apps 契约）不依赖联网决策，可与批 1 并行。

本方案文档单独一个 commit（`docs: run_python 联网口径改为「不联网」的方案`），先落档再施工。

## 七、门禁与锚点测试

口径必须被钉住，否则下次改措辞又是一次静默漂移。五个锚点测试（③ 拆成两个：一个管超时
数字，一个管联网口径）：

**① `src-tauri/src/prompts/mod.rs`**（新增）

```rust
/// run_python 的联网口径：不联网，且把联网路由到专用工具。
///
/// 三处覆盖是硬要求：主聊天提示词、子 agent profile、工具描述——只改主聊天
/// 的话，coder / executor 子 agent 仍握着一条直连暗道（三段式分工里
/// 执行与验收阶段本就不该联网）。
#[test]
fn system_prompt_forbids_run_python_network_and_routes_to_web_tools() {
    assert!(SYSTEM_PROMPT.contains("不让它访问网络"), "必须写明 run_python 不联网");
    assert!(
        SYSTEM_PROMPT.contains("web_search")
            && SYSTEM_PROMPT.contains("fetch_url")
            && SYSTEM_PROMPT.contains("http_request"),
        "必须把联网路由到 web_search / fetch_url / http_request"
    );
    assert!(
        SYSTEM_PROMPT.contains("脚本直连"),
        "必须写明理由：脚本直连没有公网闸 / 来源标注 / 目的地限制"
    );
}
```

**② `src-tauri/src/prompts/subagent.rs`**（新增，紧邻 `profiles_list_their_whitelisted_tools`）

```rust
/// 子 agent profile 必须写明 run_python 不联网——主聊天的红线不覆盖子 agent。
#[test]
fn subagent_profiles_state_run_python_has_no_network() {
    for (name, p) in [
        ("coder", PROFILE_CODER),
        ("executor", PROFILE_EXECUTOR),
        ("general", PROFILE_GENERAL),
    ] {
        assert!(
            p.contains("run_python 不联网"),
            "{name} profile 必须写明 run_python 不联网"
        );
    }
}
```

**③ `src-tauri/src/bot/registry/registry_tests.rs`**（新增，紧邻已有的
`run_python_schema_advertises_script_path_without_forcing_code`）
```rust
/// 工具描述里的超时数字 == 沙箱常量。
///
/// 数字写死在 schema 文案里（schema 是 const str，拼不进常量），历史上漂过一次
/// （文案 60s vs 常量 120s），且漂在**两处**：`run_python` 与 `ast_inspect`——
/// 后者走同一个沙箱、同一份文案位，漏改等于漏了一半。所以两条都抠出来与
/// DEFAULT_TIMEOUT_SECS 对拍，漂了当场红。
fn timeout_secs_in(desc: &str) -> Option<u64> {
    // 「默认超时 120s」与「默认 120s 超时」两种写法都认：找「默认」后第一个数字
    let mut rest = desc;
    while let Some(i) = rest.find("默认") {
        let tail = &rest[i + "默认".len()..];
        let digits: String = tail
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = digits.parse::<u64>() {
            return Some(n);
        }
        rest = tail;
    }
    None
}

#[test]
fn schema_timeout_numbers_match_sandbox_constant() {
    for (name, raw) in [
        ("run_python", super::schemas::SCHEMA_RUN_PYTHON),
        ("ast_inspect", super::schemas::SCHEMA_AST_INSPECT),
    ] {
        let v: serde_json::Value =
            serde_json::from_str(raw).unwrap_or_else(|e| panic!("{name} schema 必须是合法 JSON：{e}"));
        let desc = v["function"]["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} description 必须是字符串"));
        let n = timeout_secs_in(desc)
            .unwrap_or_else(|| panic!("{name} 描述里必须能抠出「默认 Ns 超时」"));
        assert_eq!(n, crate::py::runtime::DEFAULT_TIMEOUT_SECS, "{name} 文案超时要与常量一致");
        assert!(!desc.contains("60s"), "{name} 旧口径 60s 不得留在描述里");
    }
}

#[test]
fn run_python_schema_states_no_network() {
    let v: serde_json::Value = serde_json::from_str(super::schemas::SCHEMA_RUN_PYTHON)
        .expect("run_python schema 必须是合法 JSON");
    let desc = v["function"]["description"].as_str().expect("description 必须是字符串");
    assert!(desc.contains("不联网"), "run_python 描述必须写明不联网");
    assert!(!desc.contains("可联网"), "旧的「可联网」口径不得留在描述里");
    assert!(
        desc.contains("web_search")
            && desc.contains("fetch_url")
            && desc.contains("http_request"),
        "必须把联网路由到 web_search / fetch_url / http_request"
    );
}
```

**④ `src-tauri/src/bot/registry/registry_tests.rs`**（新增）

```rust
/// apps 程序契约：脚本自包含是交付约定，不是能力禁令。
#[test]
fn apps_script_contract_is_self_contained_not_network_banned() {
    let v: serde_json::Value = serde_json::from_str(super::schemas::SCHEMA_APPS_CREATE)
        .expect("apps_create schema 必须是合法 JSON");
    let desc = v["function"]["description"].as_str().expect("description 必须是字符串");
    assert!(!desc.contains("禁联网"), "apps 契约的「禁联网」已改为交付约定");
    assert!(desc.contains("自包含"), "apps 契约必须写明脚本自包含、不依赖外网");
}
```

门禁层面不加新规则：`tests-audit/audit_doc_claims.py` 只对拍五份实机验收文档的事件名，
不查设计文档；本次的口径真相源就是「主聊天提示词 + 子 agent profile + 工具描述」三处
文案，锚点测试已覆盖。新增一条扫 `.md` 的正则门禁会把中文表述的写法差异变成噪声门禁，
不值当。

## 八、不做清单

- **不给 `run_python` 加网络技术闸**（seccomp / 网络命名空间 / seatbelt）：本次方向是
  「不联网」，加闸与口径不矛盾，但那是独立工程——macOS seatbelt 可行（Claude Code 在
  macOS 就走这条路），**Windows 无廉价对等物**，跨平台口径分裂；且必须失败关闭（行业
  通病：Claude Code 沙箱起不来时照跑、OpenKruise 缺 sidecar 时规则不生效但调用仍返回
  成功）。另立项目，见 §十二。
- **不给子进程注入代理变量**（`HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`）：那是「代理环境
  下联网能否真跑通」的可用性问题，与口径无关，另行排期。
- **不改 `web_search` / `fetch_url` 的公网白名单闸**（`bot_web::check_public_url`）：既有
  安全边界不动，目的地限制是叠加在其上的第二道。
- **不改 apps 运行器实现**：apps 仍走同一个 py 沙箱，能力不变——本次只改契约措辞。
- **不动其余软约束**：不得读写白名单外路径、不得起子进程、绝不改系统设置。
- **不改 DEVLOG 历史条目**。
- **不新开 Windows 包**：改完先在现有环境过一遍（AGENTS.md 工作方式）。
- **不给 coder / executor 子 agent 补联网工具**：三段式分工里联网本就属于 recon 阶段，
  executor 明写「不重新调研」。本次是把执行阶段拉回设计意图，不是给执行阶段加网。

调研带来的五条「不照搬」依据见
《`docs/AGENT-SANDBOX-NETWORK-RESEARCH-2026-10-30.md`》，其中与本次方向直接相关的两条：

- **不建「代理 + 域名白名单」这套网络闸**：Claude Code 官方文档自陈其沙箱代理按客户端
  提供的域名判定、**不检查 TLS 内容**，宽域名允许存在 domain fronting 外泄路径。我们既
  无代理基础设施也无 TLS 终止能力，半套白名单（只看域名不看内容）比没有闸更危险。§五.5
  的目的地机制是**用户主动配置的信任域**（用户自己填了这个 bot 可以访问哪），性质不同于
  「任意公网放行」，但仍须在 SPEC 留档这条 domain fronting 提醒。
- **不引入容器 / microVM 隔离**：调研里走 microVM 的是云端 agent 形态，本仓是「以本机
  用户权限在用户机器上执行」，形态不匹配。

## 九、风险与缓解

| 风险 | 概率 | 缓解 |
|---|---|---|
| 断网造成能力回退（调 API、下载数据文件做不了） | 高（必然） | §五.5 的 C1–C4 是前置项，**先补能力再断网**；批 1 在批 2 之前 |
| 「不联网」只是提示词层软约束，技术层仍能联网 | 高（固有） | 本次明确认下：真正在挡的是三处统一的口径 + 专用工具的公网闸 / 来源标注 / 目的地限制 + Python 开关默认关闭 + 环境变量白名单（不继承宿主密钥，`bot/procs.rs:1173`）。**不假装提示词是硬闸**；硬闸另立项目（§十二） |
| 主聊天改了、子 agent 漏改，暗道留着 | 中 | 锚点测试②专门盯三个 profile；§五.1 的「三处覆盖」写进验收口径 |
| 专用工具被拿来当外传通道（POST body 装任意数据） | 中 | 目的地白名单 + 白名单外按次授权；单次授权不得做成「永久允许」；每次放行留审计 |
| 按次授权造成审批疲劳（Ona 实验里 agent 的绕过请求只是审批流中一次「yes」） | 中 | 白名单内默认放行，只有白名单外才弹；授权粒度绑具体目的地，不得泛化成「允许联网」 |
| apps 程序改成交付约定后仍依赖外网 | 低 | 契约措辞给出理由（「运行环境不保证外网可达」）；apps 跑在用户机器上，不可达时会直接失败而不是外传数据 |
| 外部调研的结论有时效边界 | 低 | 一手来源（Claude Code / Codex / Gemini CLI / VS Code 官方文档）在调研文档里登记了抓取时点；口径定稿不依赖它们（§二.2 的依据来自本仓自身设计） |

## 十、验收口径

1. `cd src-tauri && cargo test --lib` —— 五个新锚点测试 + 既有 `prompts` / `registry` /
   `bot_py` / `subagent` 测试全绿。
2. `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` —— 无
   警告。
3. `bash scripts/test-fast.sh` —— 快门禁过。
4. 口径对拍 grep（改完应为下列结果；`--exclude-dir=.mimosa` 必须带上——钩子基线快照里
   留着旧文案属正常）：
   ```bash
   # 三处覆盖：主聊天 + 子 agent + 工具描述（都必须有输出）
   grep -n "不让它访问网络" src-tauri/src/prompts/system.rs
   grep -n "run_python 不联网" src-tauri/src/prompts/subagent.rs
   grep -n "不联网" src-tauri/src/bot/registry/schemas.rs

   # 旧口径不得残留（以下应无输出）
   grep -rn --exclude-dir=.mimosa "可联网" src-tauri/src/prompts/system.rs src-tauri/src/bot/registry/schemas.rs
   grep -rn --exclude-dir=.mimosa "60s" src-tauri/src/bot/registry/schemas.rs src-tauri/src/py/ast.rs src-tauri/src/bot_py/tests.rs src-tauri/src/bot/services/tests/integration.rs
   grep -n "起子进程或访问网络" src-tauri/src/prompts/system.rs
   grep -rn "可联网" src/components/SettingsPage/BotApiSection.tsx

   # SPEC：只剩 :84（删除确认弹窗的 60s 超时，与 run_python 无关）——run_python 那条必须改成 120s
   grep -n "60s" SPEC.md
   ```
5. 实机抽验（可选，需本机可出网）：让模型用 `run_python` 跑一次 `urllib` 请求——应按
   提示词不主动走网络；让模型用 `fetch_url` 抓同一 URL——应走专用工具并带来源链接；让模型
   调一次外部 API（POST）——应走专用工具而非脚本。
6. 形态差异留档（写进实施记录，不写成会红的测试）：同一台机器上脚本直连回环地址可通，
   而 `fetch_url` 抓同一地址会被公网闸拒。这个差异是本次认下的已知代价，记下来备查。
7. 前端改动仅一处文案（`BotApiSection.tsx:61`），`npx tsc --noEmit` / `npm test` 照跑。

## 十一、实施结果（2026-10-31 回填）

- 实际落地：已由《`docs/RUN-PYTHON-SECURITY-REFIT-PLAN-2026-10-31.md`》整合实施——
  批 4（93b21e09）落 §五.1 三处覆盖 + §5.2 超时对齐 + §五.1④⑤ apps 契约；
  能力前置（批 1+2+3，cf57b52b/73bdfd62）先于断网落地，无真空期；
  §十二的技术闸由「另立项目」提级进本次（seatbelt，728d267d），macOS 已是硬边界。
- 与方案的偏离登记：D1 由「推荐纳入」改拍「不做」（SPEC 记已知缺口）；其余见
  整合方案 §十二。
- 实机抽验：待做（整合方案 §十.4）。

## 十二、后续排期（本次不做）

1. **技术闸：给 `run_python` 加真正的出网拒绝**。macOS 可用 seatbelt profile
   （`(version 1)(deny network-outbound)`，Claude Code 在 macOS 走同一条路）；Windows 需要
   另找机制（AppContainer / 按应用防火墙都偏重）。触发条件：出现「脚本直连外网」的实机
   工单，或用户明确要求把它变成硬边界。实现时必须照搬调研里的教训：**闸未生效时报错而非
   静默放行**（默认失败关闭），否则只是又添一个 fail-open 开关。
2. **域名级按次授权的形态细化**（§五.5 的 C3 落地后）：授权要绑具体目的地与方法，不得
   泛化成「允许联网」。
3. **子 agent 联网面的复查**：三段式分工里 coder / executor 不联网是对的，但如果将来
   executor 需要查资料，应走 recon 或专用工具，而不是给 `run_python` 开口子。

另有一条只登记不排期：若发生「脚本把本机敏感数据发往外部」的实机事故，再评估给
`run_python` 加公网白名单闸。届时必须按 §八 的约束做——deny-rest 兜底、allow 优先于
deny、闸未生效时报错而非静默放行、以及「只看域名不看 TLS 内容的白名单单独用等于没闸」
这条硬提醒。
