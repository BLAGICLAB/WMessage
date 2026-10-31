# 架构事实表：工具的跑位、闸与强制缺口（2026-10-30）

> 本文是**现状盘点**（不是方案，不含待办）。落地后的行为以代码为准。
>
> 用途：评估任何新闸（目的地白名单 / seatbelt / rlimit 补齐 / 子 agent 联网面）
> 之前先查这张表——尤其是 §五「强制缺口清单」。
>
> 相关文档：《`docs/RUN-PYTHON-NETWORK-POLICY-2026-10-30.md`》（run_python 断网口径）、
> 《`docs/HTTP-REQUEST-TOOL-DESIGN-2026-10-30.md`》（专用 HTTP 工具与目的地闸）。

## 一、读法：三条口径

### 1.1 「沙箱」在本仓只有一个含义

指 `src-tauri/src/py/runtime.rs` 那套 Python 执行沙箱。它的维度是**资源 + 生命周期**：
内存限额、CPU 限额、进程数限额、时长、输出上限、独立临时目录、进程组连坐杀。

它**不是**能力维度的隔离：不隔离文件系统、不隔离网络、不禁止起子进程。所以「run_python 在
沙箱里」这句话容易误读成「它被关住了」——实际是「它只是被限量了」。

### 1.2 闸 ≠ 强制

| | 闸（gate） | 强制（enforcement） |
|---|---|---|
| 形态 | 判定「要不要做」 | 「做了也拦得住」 |
| 载体 | dispatch 的闸序 + 各工具本体的闸 | OS 级机制（rlimit / Job / seatbelt / netns） |
| 可被绕过 | 逻辑错误、提示词失守、用户误点 | 绕不过（前提是机制本身没被关掉） |
| 本仓覆盖 | 几乎所有工具 | **只有 py 执行一条路径，且只在资源维度** |

### 1.3 本文的四列

**跑位**（在哪执行）→ **数据边界**（能碰到什么）→ **闸**（判定层）→ **强制层**（有没有 OS 级兜底）。

## 二、闸序：dispatch 的执行链（`src-tauri/src/bot/dispatch.rs`）

工具调用按固定顺序过闸。**顺序本身就是设计**：越靠前越便宜、越通用。

| 序 | 闸 | 位置 | 判定 | 失败方向 | 审计事件 |
|---|---|---|---|---|---|
| 0 | `tool.call` 记账 | `dispatch.rs:481` | — | — | `tool.call` |
| 0.5 | L4 `pre_tool` 钩子层 | `dispatch.rs:270` `run_pre_tool_layer` | 外部注册的钩子只读拦截 | **钩子返 Err / panic → fail-open 放行**（只记审计）；只有 `Interrupt` 才拒 | `pre_tool.hook_failed` / `pre_tool.hook_interrupt` |
| 0.55 | 活动技能 `allowed-tools` | `dispatch.rs:498` | 技能作者自限的工具集合 | 硬拒 | `skill.allowed_tools_violation` |
| 0.6 | per-tool 规则表 | `dispatch.rs:323` `tool_rule_gate` → `bot_fs::tool_rule_for` | 设置页授权卡：`deny` / `ask` / `allow` | `deny` 硬拒；`ask` 弹确认，被拒则拒 | `tool_rule.hit` |
| 2 | Skill 步骤钩子 | `dispatch.rs:535` `skill_on_step` | 活动技能的计数 / 熔断 | 熔断 = Denied（不是 Warn） | `skill_on_step_error` |
| 1.6 | 子 agent 会话白名单 | `dispatch.rs:557` | 判据是 `ctx.tools`（profile ∩ per-onion 的交集） | 硬拒 | `subagent_whitelist_deny` |
| 3 | 入参 JSON 闸 | dispatch 内 | 参数合法 | 硬拒 | — |
| 4 | **工具本体的闸** | 各工具自己 | 路径 / 目的地 / 开关 / 确认弹窗 | 拒 | 各工具自记 |

**两条必须记住的结构事实：**

1. **dispatch 层只管「能不能调这个工具」，管不了「这个工具能碰什么」。** 后者分散在各工具
   本体里各写各的：`bot_fs` 的路径闸、`bot_web` 的公网闸、`py::commands::ensure_py_enabled`
   的 Python 开关、`bot/services` 的端口闸……**这正是口径容易漂移的根源**——新增一个能碰
   同一类资源的工具，旧工具的闸不会自动套用到它身上。
2. dispatch 层的早退恰好 **7 处**（`dispatch/pre_tool_hook_tests.rs:404-412` 用计数断言锁
   住）：pre_tool 钩子拒绝 / allowed-tools 集合外 / 规则 deny / 确认被拒 / skill 熔断 /
   子 agent 白名单外 / arguments 非法 JSON。工具本体的拒绝不在这 7 处计数里。

## 三、事实表 A：65 个工具逐条

> 工具数 65 = `bot::registry::TOOLS_TABLE` 条目数（`registry/schemas.rs` 头部自述）；
> `read_own_card` / `write_artifact_file` 仅子 agent 可见，不进主 agent 清单。

### G1 任务卡（10 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `query_tasks` / `query_single_task` | 宿主进程内 | 读本机 DB | 子 agent 白名单（只读档可用） | 无 |
| `create_task` / `complete_task` / `edit_task` / `add_subtask` / `toggle_subtask` / `remove_subtask` / `link_file_to_task` | 宿主进程内 | 写本机 DB | **主状态写全部不在子 agent 白名单**（dispatch 1.6 兜底） | 无 |
| `delete_task` | 宿主进程内 | 移回收站，可恢复 | 删前弹确认，60s 无响应默认拒（`SPEC.md:303`） | 无 |

### G2 本地文件读（5 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `read_text_file` / `grep_files` / `list_files` | 宿主进程内 | **读白名单内任意路径**：桌面/下载/文档 + 任务卡绑定文件夹 + 设置页 `allowedDirs` | `bot_fs::resolve_with_perm`：白名单外 `strict` 拒 / `ask` 弹确认 / `yolo` 放行 | 无（文件系统隔离不存在） |
| `extract_document` | 宿主进程内 | 同上；PDF 走 `pypdf`/PyMuPDF | 同上 | 无 |
| `ocr_image` | 宿主进程内（macOS Vision / Windows PP-OCRv6 本地推理） | 只收**本地路径**，网络路径在入口直接拒 | 同上 + **隐私红线：图片绝不上传外网**（`ocr.rs:3-5`、`schemas.rs:66`） | 无（但这条红线靠「不实现网络路径」成立） |

### G3 本地文件写（7 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `edit_file` / `write_file` | 宿主进程内 | **可写白名单**：AI_Gen_Files + 任务卡绑定文件夹 + `allowedDirs`。桌面/下载/文档**只读不可写** | 白名单外按 perm_mode 三分支；覆盖已有文件弹确认 | 无 |
| `manage_file` | 宿主进程内 | copy/move/delete；源在读白名单、目标在可写白名单 | delete 删前弹确认（60s 默认拒）；目标已存在不覆盖；符号链接不跟随 | 无（delete 走系统回收站，可恢复） |
| `unpack_archive` / `pack_archive` | 宿主进程内 | 解压目标默认可写目录 | 条目路径逐条校验（`..` / 绝对路径 / 盘符 / 软链跳过并列出）；单文件 64MB / 整包 256MB / 条目 5000 上限 | 无 |
| `copy_file_to_clipboard` | 宿主进程内 | 读白名单内文件 → 系统剪贴板 | 读白名单 | 无 |
| `write_artifact_file` | 宿主进程内 | 产物目录 | 子 agent 专属 | 无 |

### G4 文档产出（5 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `create_word` / `create_word_revisions` / `create_excel` / `create_ppt` / `create_pdf` | 宿主进程内 | 产物落 AI_Gen_Files | 修订模式写副本不覆盖原文件；Excel 全部可算值用公式 | 无 |

### G5 联网（2 个现有）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `web_search` | 宿主进程内（reqwest） | **产品自有出口**：Tavily / Brave API，或 Bing + 百度网页抓取 | 结果体 2MB 线；输出 6000 字 | 无（目的地是产品/用户配的 provider） |
| `fetch_url` | 宿主进程内（reqwest） | **任意公网 http/https** | ① 公网闸 `check_public_url`（协议 + 主机 + DNS 全部 IP 都要公网，含环回/私网/链路本地 `169.254.x`）；② 钉 IP 防 DNS TOCTOU；③ 不跟随自动重定向、手动逐跳校验 Location；④ 内容类型闸（只收 html/xml/text，**JSON/PDF 被拒**）；⑤ 10MB 传输闸（流式封顶，不信 Content-Length）；⑥ 30K 字切片 + offset 续读 | **无目的地白名单**（任何公网都放行）——本次要补的就是这条 |

### G6 Python 执行（2 个工具 + 1 类程序）

| 能力 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `run_python` | **py 沙箱子进程** | **用户权限读任意文件**；可联网；可起子进程（被 NPROC 限量）；临时目录内写 | ① `ensure_py_enabled`（设置页开关默认关闭，yolo 免开关）；② 提示词红线；③ 沙箱临时目录只加读侧 | **有（资源维度）**：AS（timeout×8MB，钳 256MB–2GB）、CPU（timeout+10s）、NPROC（用户进程基数+64）、时长（默认 120s / 上限 600s）、输出 64KB、进程组连坐杀、Windows Job Object。**无文件隔离、无网络隔离** |
| `ast_inspect` | 同一 py 沙箱 | 读 `.py` 文件（路径先过白名单闸），**只解析不执行** | 同上 | 同上（但它没有出网需求，禁网是零成本） |
| apps 上架程序 | 同一 py 沙箱（`apps/mod.rs:151`） | 同 `run_python` | 同一 Python 开关（`apps/commands.rs:64`：「同一执行面同一口径」） | 同上，超时上限 1800s（`runtime.rs:42`） |

### G7 技能（4 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `use_skill` | 宿主进程内 | 读已装技能（技能内可用工具受 `allowed-tools` 声明限制） | dispatch 0.55 + 0.6（`use_skill` 两处豁免） | 无 |
| `create_skill` | 宿主进程内 | 写技能目录 | **准入核**（frontmatter / intents 危险词 / intents 模式可编译且不过宽 / `mode=auto` 必须含 `## Step N:` 块）+ **用户确认** | 无 |
| `install_skill` | 宿主进程内（**reqwest 下载，不 spawn git**） | 写技能目录 | 同准入核 + 用户确认；来源限 GitHub / SkillHub / zip 直链 | 无（下载内容不校验签名） |
| `search_skills` | 宿主进程内 | 出网到 SkillHub | — | 无 |

### G8 子 agent（4 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `spawn_subagent` / `check_subagent` / `cancel_subagent` | 宿主进程内（起子会话） | 子 agent 拿到的工具 = profile ∩ per-onion `allowed` | dispatch 1.6 兜底（**递归禁用**：子 agent 调 spawn/check/cancel 一律拒） | 无 |
| `read_own_card` | 宿主进程内 | 读自己的任务卡 | 子 agent 专属 | 无 |

### G9 桌面与系统（10 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `list_windows` / `activate_window` / `ui_inspect` / `ui_click` / `ui_type` / `wait_for_element` | 宿主进程内（**驱动别的应用**） | **能操作已登录窗口**——点「发送 / 提交 / 支付 / 删除」 | ① 独立开关「允许机器人操作桌面界面」默认关闭；② 这类按钮前弹确认，用户拒绝不重试；③ 密码 / 验证码不代填 | 无 |
| `screenshot` | **子进程**：macOS `screencapture` / Windows `silent_cmd` 拉 PowerShell | 主显示器全屏 | 屏幕录制权限；PNG 落 AI_Gen_Files | 无（命令名固定，不是模型给的） |
| `open_url` | 宿主进程内 | 把链接丢给系统浏览器（仅公网 http/https） | 公网地址 | 无 |
| `reveal_path` | 宿主进程内 | 在访达/资源管理器显示文件 | 白名单目录内 | 无 |
| `clipboard_write` | 宿主进程内 | 写系统剪贴板 | — | 无 |

### G10 受管服务（3 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `start_service` / `stop_service` / `list_services` | **独立子进程**，长驻 | **跑用户指定的命令**（白名单启动器） | ① 启动器白名单；② cwd 过 `bot_fs` 路径闸；③ **端口/监听地址闸**：默认只许 loopback，放开非 loopback 要单独开关；④ 资源观测告警（RSS/CPU 超阈值出声，不自动杀） | **无 rlimit**（SPEC 明写「不继承 run_python 的 rlimit」——Linux 上 `RLIMIT_AS=480MB` 会让 node 启动即失败）。Windows 侧复用 `py::runtime::win_job` 的 `create_kill_on_close()`，但那只保证「宿主一死就杀光」，无内存/CPU 限额 |

### G11 记忆 / 计划 / 问答（7 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `remember_fact` / `recall_facts` / `record_lesson` | 宿主进程内 | 写/读本机 DB | 子 agent 白名单 | 无 |
| `get_current_time` / `update_plan` / `lookup_receipt` / `ask_user` | 宿主进程内 | 无外部副作用 | 子 agent 白名单（`update_plan` 等） | 无 |

### G12 提醒（3 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `schedule_reminder` / `list_reminders` / `cancel_reminder` | 宿主进程内 | 写 DB + 到点发系统通知 | — | 无。**提醒到点只说话，不执行任何内容**；执行是定时作业那条路 |

### G13 程序管理（3 个）

| 工具 | 跑位 | 数据边界 | 闸 | 强制层 |
|---|---|---|---|---|
| `apps_list` | 宿主进程内 | 读程序登记表 | — | 无 |
| `apps_create` / `apps_update_files` | 宿主进程内（写脚本） | 写程序目录 | 同一 Python 开关（`apps/commands.rs:64`） | 无 |

## 四、事实表 B：py 沙箱的能力矩阵

| 维度 | 管不管 | 实现 | 备注 |
|---|---|---|---|
| 内存 | ✅ | `RLIMIT_AS` = timeout×8MB，钳 256MB–2GB（`runtime.rs:58`） | Windows 走 Job Object |
| CPU | ✅ | `RLIMIT_CPU` = timeout+10s（`runtime.rs:65`） | |
| 进程数 | ⚠️ 限量非禁止 | `RLIMIT_NPROC` = 用户进程基数 + 64（`runtime.rs:69-80`） | **按用户计数**；单个子进程照样起得来（`curl` 能跑） |
| 时长 | ✅ | 默认 120s / 上限 600s；apps 1800s | 超时强杀进程组 |
| 输出量 | ✅ | 64KB | |
| 落盘位置 | ⚠️ 弱 | 独立临时目录（`py_runs_root()` = temp/wmessage-py-runs/<uuid>） | 目录只加**读侧**白名单——模型不能往沙箱里写；但沙箱外任何可写路径都能写 |
| 文件系统 | ❌ | 无 | 以当前用户权限读任意文件 |
| 网络 | ❌ | 无 | 可直连任意地址 |
| 子进程 | ❌ | 无禁止 | 被 NPROC / CPU / 进程组连坐间接约束 |
| 系统设置 | ❌ | 无 | 提示词说「绝不」，技术层不管 |

**探测与降级的诚实性**（值得单独记）：`SETRLIMIT_PROBE_SCRIPT` 逐资源探测可用性，探针取值
就是生产上限（由 argv 从 Rust 常量传入，不在脚本文本里写死），探测值被拒 → 从严判
Unavailable 留痕（丢限额但诚实），绝不误判「可用」静默失效（`runtime.rs:688-694`）。

## 五、强制缺口清单（按「能力面 × 强制缺口」排）

| 能力 | 能力面 | 强制层 | 缺口 |
|---|---|---|---|
| `run_python` | 读任意文件、执行任意代码、可直连任意地址 | 资源限额 | **无文件隔离、无网络隔离、无子进程禁止** |
| `start_service` | 起长驻进程、可监听端口 | 端口闸 + 资源告警 | **无 rlimit**——长驻进程不受内存/CPU 限额约束 |
| MCP stdio 服务器 | 跑用户配置的程序 | 启动器白名单 + cwd 路径闸 | **无 rlimit、无 seccomp**；程序本体由用户在设置页指定 |
| `ui_click` | 操作已登录窗口（发送 / 支付 / 删除） | 危险按钮弹确认 | **确认可被跳过**（提示词说「不反复重试」，但闸本身拦不住一次误点） |
| `install_skill` | 下载并落盘可执行内容 | 准入核 + 用户确认 | 无签名校验；准入核查的是意图模式不是代码行为 |
| `fetch_url` | 任意公网 + 查询串外传 | 公网闸（挡私网/环回/元数据） | **无目的地白名单**——外网任意去，GET 查询串可夹带数据 |
| `write_file` / `edit_file` | 写白名单外路径 | perm_mode 三分支 | ask 模式下靠弹窗，非强制 |
| `write_artifact_file` / `create_*` | 产物落盘 | 路径限定在 AI_Gen_Files | 范围本身窄，风险低 |

**这张表的读法**：`run_python` 是**唯一被限额的那条**，也是**能力最窄的那条**（只能用
stdlib）。按「能力面 × 强制缺口」排，真正排在前面的是 `start_service` 与 MCP stdio——它们
既没有 rlimit，也没有能力维度的限制。

## 六、从这张表得出的三条结论

1. **只有 py 执行这一条路径有 OS 级强制，且只在资源维度。** 本仓至今**没有任何能力维度的
   沙箱**（文件 / 网络 / 进程都没被能力级关住）。给 Python 执行加 seatbelt 断网，会是
   本仓第一条能力维度的沙箱——它不是给已有沙箱补洞，是新建一层维度。
2. **闸的分布是分散的，闸序是集中的。** dispatch 管「能不能调」，工具本体管「能碰什么」。
   新增一个能碰同类资源的工具时，旧闸不会自动套用——这是口径漂移的结构性来源，也解释了
   为什么「主聊天提示词改了、子 agent 漏改」这类问题会反复出现。
3. **fail-open 有两处，必须记住**：① L4 钩子返 Err / panic → 放行（只记审计）；② 受管
   服务不继承 rlimit（刻意为之：否则会误杀构建中的 dev server）。两处都是有意设计，但都
   意味着「闸看起来在，实际不兜底」。

## 七、核查口径与已知空白

- 工具清单以 `bot::registry::TOOLS_TABLE` 为准（65 条）；本文按 13 组归类，组内同名工具
  共用一行闸与边界。
- 闸序以 `bot/dispatch.rs` 的 `execute_tool_impl` 为准；早退点数 7 由
  `dispatch/pre_tool_hook_tests.rs` 的计数断言锁住。
- **已知空白**：MCP 侧配置（用户配的 server 列表与其 URL/命令）与本文的「目的地」概念
  尚未对齐——MCP 目的地是用户配的，天然属于「产品/用户自有出口」，但本仓没有把
  「用户配的 MCP 目的地」与「将来 http_request 的目的地白名单」统一成一张表。方案
  《`docs/HTTP-REQUEST-TOOL-DESIGN-2026-10-30.md`》§3.1 的两层分类是第一步，但 MCP 侧
  还没接进去。
- `src-tauri/src/.mimosa/` 是钩子基线快照（已 gitignore），里面留着旧文案属正常，核查时
  要排除。

## 八、维护规则

这张表会过期。改动下列任一项时**必须同步本文**：

- 工具表增删（`registry/schemas.rs` 的 `TOOLS_TABLE`）
- 闸序变更（`bot/dispatch.rs` 的 `execute_tool_impl`）
- 某个工具的跑位改变（从 in-process 变成起进程，或反过来）
- 某个工具新增/移除闸（尤其是「这个工具能碰什么」那一层）
- py 沙箱的任一限额值或探测口径变更（`py/runtime.rs`）

同步时只改事实，不改判断；判断留在方案文档里。
