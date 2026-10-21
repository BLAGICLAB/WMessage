# 受管长驻服务层设计（Managed Service Lifecycle）

> 状态：设计稿，待实施
> 基线：git HEAD `6d79faa3`
> 关联调研：`.mavis/mavis-deep-research/20261021_165500_agent-sandbox-mcp-lifecycle/final_turn_001.md`

---

## 0. 一句话结论

`run_python` 是**有界作业**，它连坐杀进程组是对的，不动它；缺的是**宿主托管的长驻服务层**——
由宿主（Rust 侧）直接 spawn 受管进程、登记在 `AppState`、用 SIGINT→SIGTERM→SIGKILL 降级阶梯停止、
递归回收脱离进程组的子孙、应用退出时统一收干净。新增三个工具：`start_service` / `stop_service` / `list_services`。

**服务层不继承 `run_python` 的任何 rlimit**——这不是口味问题，是实测事实（§5）。

---

## 1. 问题：缺口在哪

### 1.1 现象

skill 或 MCP 想在本机起一个活过本次工具调用的 node 进程（node-based MCP server、dev server、
watcher、隧道），当前只有一条路：`run_python` 里 `subprocess` 起 node。

这条路必死。四道杀手依次生效：

| # | 杀手 | 位置 | 触发条件 |
|---|---|---|---|
| 1 | `process_group(0)` + 连坐杀组 | `py/runtime.rs:789`、`:205-229` | 作业超时 / 失败 → `kill -9 -<pgid>` |
| 2 | drain 超时连坐 | `py/runtime.rs:941-946` | 孙进程占着 stdout 管道不放 → 2s 收不齐 → 整组强杀 |
| 3 | 跑完删目录 | `py/runtime.rs:957` | `remove_dir_all(dir)`，node 的 cwd/产物被抹掉 |
| 4 | 父进程看门狗 | `py/runtime.rs:322-330` | ppid ≤ 1 → `_exit(137)` |

实测复刻 `run_python_at` 时序（A 持管道+杀组 / B 持管道+不杀组 / C 不持管道+不杀组）：
**node 完全可以活过运行，唯一杀手是「连坐杀组」**。也就是说能力不缺，缺的是**归属**：
没有一层来声明「这个进程归我管、我知道它是谁、我说停就停、退出时我收干净」。

### 1.2 缺口的准确定性

业界的共识是：**沙箱隔离层**和**服务生命周期层**是两件事，把它们压成一层就会两头不讨好。

- Claude Code 官方 sandboxing 文档：沙箱只包 shell，*"Claude's file tools, MCP servers, and hooks run outside it"*。
  MCP server 在沙箱外以完整权限运行，宿主负责它的生命周期。
- MCP 规范 2025-06-18：stdio 传输 = *"The client launches the MCP server as a subprocess."*
  ——**客户端（宿主）是归宿主管**。规范不规定 kill 策略，那部分是实现者的事。
- Codex 源码（基线 commit `d58d0e58`）：`SandboxManager` 与 `UnifiedExecProcessManager` **两职责分离**。

wmessage 目前只有沙箱层（`py/runtime.rs`），没有服务层。`bot/mcp/manager.rs` 里那套算半个服务层，
但它只服务 MCP 协议进程（rmcp 持管道、Drop 兜底 kill、无降级阶梯），skill 内部要起的裸进程无处安放。

### 1.3 已被否决的两条路

| 方案 | 判定 | 理由 |
|---|---|---|
| 方案 1：node MCP server 配到设置页 | 可行但**救不了本需求** | 只能起"预先配置好的"node MCP server；skill 内部动态起服务仍然无路可走。可作为**配套项**（§16 P2） |
| 方案 3：裸 `setsid` 逃逸 | **不建议** | 沙箱内代码自行 daemonize 被业界当**缺陷**（Claude Code 2.1.212 / 2.1.257 明确把 setsid/timeout 逃逸当 bug 修），wmessage 把它记成"已知 trade-off 不修"（`DEVLOG.md:4150`）。照抄会站到少数派一边，且宿主对逃逸进程**完全无知**——连"能停它"都做不到 |

**本方案 = 方案 2：宿主托管的长驻进程层。**

---

## 2. 目标与非目标

### 目标

1. skill / 模型能在本机起一个**长驻**进程（node / deno / python 等白名单启动器），活过本次工具调用。
2. 宿主**知道**它：登记表里有 id / pid / 状态 / 输出尾部，可查可列。
3. 宿主**能停它**：显式停止 + 降级信号阶梯 + 脱离进程组的子孙一并回收。
4. 宿主**退出时收干净**：不留孤儿、不留僵尸、不留半截文件。
5. 长驻进程**不被沙箱式限额误杀**（§5）。
6. 权限面与 MCP 侧**同源**：同一份启动器白名单、同一套路径授权闸、同一套 yolo 免开关口径。

### 非目标（明确不做）

| 不做 | 理由 |
|---|---|
| 改 `run_python` 的杀进程力度 | 有界作业连坐杀组是对的，动了会开一个逃逸口子 |
| 崩溃自动重启 | restart loop 是经典 bug 源；需要时用户自己重启 |
| 端口占用/监听地址闸 | 审计记全 argv + `list_services` 展示 stdout 尾部，用户看得见。属硬化项，不预支（§16） |
| 自定义 env | 密钥边界 + YAGNI。服务**继承宿主环境**，不开放注入 |
| 会话结束自动回收服务 | wmessage 没有"会话结束"单一事件源；误杀跨会话复用的服务比留着的代价大（§13.4） |
| 把停止阶梯抽成公共模块 | 第二个调用方（MCP stdio）本期不接，抽象会变成空壳（AGENTS.md：不加投机抽象） |
| 服务日志进 `AI_Gen_Files` 产物通道 | 服务日志不是"用户要的文件"，是诊断留存 |

---

## 3. 业界口径（本方案抄的三条）

这三条是反复出现的共识，本方案直接采纳，不再论证：

1. **超时不该直接杀，可先转后台。**
   Claude Code 官方 interactive-mode 文档：前台命令超时是**移入后台**而不是杀掉，
   并提供 `run_in_background: true` / `/tasks` / `TaskStop` / Ctrl+B；官方列出的后台化用途里
   **明确包含 dev server**。
   → 对应本方案：长驻服务本来就是"进后台"，不是"跑超时了"。

2. **停止走 SIGINT → SIGTERM → SIGKILL 降级阶梯。**
   Claude Code 官方 mcp 文档：macOS/Linux = SIGINT → 100ms → SIGTERM → 400ms → SIGKILL；
   Windows = 先关 stdin → 300ms → 结束进程树。
   → 本方案**逐毫秒照抄**（§10）。

3. **脱离进程组的子孙要一并回收。**
   Claude Code 官方 interactive-mode 文档：*"processes that detached from the task's shell,
   such as ones started under `setsid` or `timeout`, stop too"*。
   → 本方案在停止时**递归收集 ppid 树**并补杀（§10.3）。

### 一处诚实的补充

Claude Code 自身这洞也没堵完（claudeissues #102511，v2.1.300 状态 Open）；
且其 CHANGELOG 直连失败（curl exit 35 / web_fetch 失败），上面两个版本号取自检索到的条目引文，
**行为本身以官方 interactive-mode 文档原文为准**。

---

## 4. 关键取舍（六条已定）

| # | 取舍 | 理由 |
|---|---|---|
| 1 | 保留 `run_python` 现状不动 | 有界作业、连坐杀组是对的；不动它就不开逃逸口子 |
| 2 | 受管进程由**宿主直接 spawn**，不经 Python | 不继承 rlimit（§5）；不经 `py/runtime.rs` 的组杀/目录删除路径 |
| 3 | stdout/stderr **落文件**，不占父进程管道 | 绕开 drain 超时那条连坐杀组路径（`py/runtime.rs:941-946`）；文件还能在进程死后读 |
| 4 | 权限闸**复用** `STDIO_LAUNCH_ALLOWLIST` + 逐臂字面量构造 | `bot/mcp/config.rs:49-51` 注释已声明"单一事实源"；不新增白名单、不新增依赖、不走 shell |
| 5 | 登记表放 `AppState`，不放全局 static | `app_state.rs` 已是全局可变状态的单一入口；`skill_runs` 是同类先例 |
| 6 | 停止阶梯**先做在服务层私有**，不上提为公共模块 | 第二个调用方（MCP stdio 接同一阶梯）本期不接；提前抽象 = 违反 AGENTS.md 的 YAGNI |

---

## 5. 限额继承口径（核心待答问题）

### 5.1 问题

`run_python` 在 spawn 前用 `pre_exec` 设了 `RLIMIT_AS` / `RLIMIT_CPU`
（`py/runtime.rs:800-825`）：

- `RLIMIT_AS = mem_limit_bytes(timeout)`（`py/runtime.rs:55-60`）= `timeout × 8MB`，钳在 256MB–2GB。
  默认 timeout 60s → **480MB**。
- `RLIMIT_CPU = cpu_limit_secs(timeout)`（`py/runtime.rs:62-64`）= `timeout + 10`。
  默认 timeout 60s → **70 秒 CPU**。

如果服务进程继承这两个限额会怎样？**实测如下**（本机 macOS arm64，node v26.5.0）。

#### 实测 A：RLIMIT_CPU 被 node 继承，且触发 SIGXCPU

```
$ python3 wm_limit_probe.py
parent RLIMIT_CPU = (2, 9223372036854775807)
child says: node ulimit -t = 2
burn node rc=-24 (SIGXCPU) elapsed=2.0s
```

即：父进程设 2 秒 CPU 限额 → **node 子进程 `ulimit -t` 也是 2** → 一个正常烧 CPU 的 node
在 2.0 秒时收到 **SIGXCPU（rc=-24）** 直接死。

> node 默认不装 SIGXCPU handler，默认动作 = 终止（core dump）。**没有任何机会优雅收尾。**

**推论（决定性）**：`run_python` 的 `RLIMIT_CPU = timeout + 10`（默认 70s）对长驻服务是致命的。
一个 `vite dev` 首次构建、一次 `tsc --build`、一个 `webpack` 全量打包，CPU 累计很容易过 70 秒 →
服务在**用户正要用它的时候**被 SIGXCPU 杀掉，而且宿主完全不知道它为什么死了（没有退出码语义，只有信号）。

#### 实测 B：node 的虚拟内存足迹远超任何可能的 RLIMIT_AS

```
$ bash wm_vsz_probe.sh
32120 436127312  42432 node        # VSZ = 436127312 KB ≈ 416 GiB，RSS 仅 42 MB
$ ulimit -v 524288
/bin/bash: line 11: ulimit: virtual memory: cannot modify limit: Invalid argument
```

两件事：

1. **本机（macOS）内核直接拒 `RLIMIT_AS`**（`Invalid argument`）。这正是 `py/runtime.rs:588-761`
   那套 `setrlimit_probe_cached` 存在的原因——裸 macOS 上 AS 探测结果是 `Unavailable`，
   所以 `pre_exec` **不会**设 AS，只设 CPU（`py/runtime.rs:797-798` 逐资源判断）。
2. **但 Linux 上 `RLIMIT_AS` 是可设的**，探测会返回 `Available`，于是 `RLIMIT_AS = 480MB` 真的会被设上。
   而 node 的虚拟地址空间预留是 **10²–10³ GB 量级**（本机实测 VSZ 416 GiB，V8 一次性预留大段虚拟地址空间，
   RSS 才 42MB——RSS 小不代表 VSZ 小）。`RLIMIT_AS` 卡的是**虚拟地址空间总量**，
   480MB 远低于 node 的启动需求。

**推论（决定性）**：在 Linux 上，"服务从 `run_python` 里起"会因为 `RLIMIT_AS` **启动即失败**；
在 macOS 上则会因为 `RLIMIT_CPU` **跑到一定 CPU 量就死**。两条路都堵死。

### 5.2 结论：服务层与沙箱层**不共享限额**

| | 沙箱层（`run_python`） | 服务层（本方案） |
|---|---|---|
| 进程模型 | 有界作业，跑完即弃 | 长驻，宿主托管 |
| 内存 | `RLIMIT_AS`（可设时） | **不设**（macOS 不允许 / Linux 会杀 node） |
| CPU | `RLIMIT_CPU = timeout+10` | **不设**（会 SIGXCPU 杀正在干活的 dev server） |
| 时长 | `timeout`（默认 60s，上限 1800s） | 无固定上限，生命周期由停止/退出决定 |
| 终止 | 连坐杀组（正确） | 降级阶梯 + 子孙回收 |
| 归属 | 无（跑完就删目录） | 登记表 + id + 显式停止 |

**"限额"的语义在两层之间换了东西**：

- 沙箱层的限额回答的是「**这次调用最多烧多少**」——作业的预算。
- 服务层的约束回答的是「**这个进程归谁、什么时候必须死、失控了怎么办**」——生命周期。

### 5.3 服务层用什么替代限额

四类约束，全部**跨平台可执行**（不依赖内核 rlimit 支持）：

| 约束 | 机制 | 性质 | 默认值 |
|---|---|---|---|
| **输出上限** | stdout/stderr 落 `O_APPEND` 文件；宿主巡检（默认 30s）发现超限 → 保留尾部 N/2 字节重写 | **硬**（不超） | 8MB / 流 |
| **并发上限** | 同时存活的受管服务数超限 → `start_service` 直接拒绝 | **硬** | 4 个 |
| **生命周期上限** | 显式 `stop_service` / 应用退出必杀 | **硬** | — |
| **资源观测** | 宿主定期采样 RSS / CPU 时间；超阈值 → **告警**（审计 + 工具返回里带告警），**不自动杀** | **软** | 单服务 RSS 512MB / 合计 RSS 1GB / CPU 跑满持续 5min（阈值实测见 §5.4） |

**采样口径是整棵进程树**（组长 + 全部后代）合计：dev server 的活多半在子进程里
（vite 拉起的 esbuild），只看组长会漏掉真正在烧的那个。

**CPU 阈值实现成 95% 而不是 100%**：占用率是两次采样之差算出来的，采样时刻与进程
记账有误差，真跑满时算出来是 99.9x%——阈值卡 100% 就永远差一点，永远不告警。

**为什么内存只告警不杀**：macOS 没有任何等价的 per-process 内存硬限额
（`RLIMIT_AS` 被内核拒；cgroup 是 Linux 专有）；Linux 上有 cgroup v2 但跨平台不一致。
"跨平台口径一致"在这个仓库里比"单平台更严"更重要（现有 `setrlimit` 探测机制就是为此设计的）。
把"杀服务"的决策权交给用户（`stop_service`）+ 退出必杀，比一个在 macOS 上不存在的限额更诚实。

**为什么 CPU 只告警不杀**：见 §5.1——CPU 上限对"正在构建的 dev server"是误杀。
真要防跑飞，宿主侧的采样告警 + 用户能停，比内核信号更合适。

### 5.4 内存占用（实测）

**一句话：宿主这一层几乎不占内存（< 1MB），内存全部在被托管的进程里。**

#### 被托管进程侧的占用（本机 macOS arm64 实测）

| 进程 | RSS | VSZ | 进程数 |
|---|---|---|---|
| 最小 node http server（`node -e` 起一个 listen） | **44.1 MB** | 415.9 GB | 1 |
| 本仓库真实 `vite dev`（vite 7.3.6，仓库自带 node_modules） | **127.0 MB** | 424.1 GB | 1 |
| └ vite 拉起的 esbuild 服务子进程 | 17.6 MB | — | 1 |
| **vite 整棵进程树合计** | **144.6 MB** | — | **2** |

> **VSZ 大 ≠ 占物理内存。** node 的 VSZ 是 416 GB（V8 一次性预留大段虚拟地址空间），
> 但 RSS 只有 44–127 MB。所以「要不要给服务加内存限额」这个问题**用 RSS 回答，不拿 VSZ 回答**。
> 反过来说：正因为 VSZ 是 416 GB，**任何 `RLIMIT_AS` 级别的虚拟地址空间上限都会让 node 启动即失败**——
> 这不是「可以设但要设大」，是「设了就没了」。§5.2 的结论在内存这一侧同样成立。

#### 宿主（wmessage）侧新增的占用

| 项 | 实测 / 估算 | 说明 |
|---|---|---|
| watcher 线程 | **≈ 0.1 KB RSS / 个**（实测 200 个线程 RSS 增量 ≈ 0） | 4 个服务 = 4 个线程，栈按需提交，不预占 2MB 虚拟栈 |
| 登记表条目 | ~几百字节 / 条 × 4 | `HashMap<String, ManagedService>`，字符串与路径为主 |
| `ps` 快照（停止时的 ppid 树） | 本机 757 进程 → 表 ~6KB + 瞬时输出串 ~30KB | 只在停止时拍，低频、瞬时 |
| stdout/stderr 日志文件 | 8MB × 2 流 × 4 服务 = **64MB** | **页缓存，可回收，不计入进程 RSS**（实测写 8MB 文件不涨进程 RSS） |
| **宿主合计** | **< 1 MB**（不含页缓存） | 相比一个 vite dev server 的 127 MB，可以忽略 |

#### 由此得到的内存预算

| 场景 | 内存 |
|---|---|
| 1 个最小 node 服务 | ~44 MB |
| 1 个 vite dev server（含 esbuild 子进程） | ~145 MB |
| **并发上限打满（4 个 vite dev server）** | **~580 MB** |
| 4 个重负载（编译型）服务 | 可达 ~2 GB（此时**该由告警告诉用户**，而不是自动杀） |

**告警阈值按实测反推**：

- 单服务 RSS **512MB**：正常 vite dev server 是 145MB，阈值在它上面 3.5 倍——能抓住真正失控的进程，又不会对正常 dev server 误报。
- **全部服务合计 RSS 1GB**：4 × 145MB = 580MB 属正常（不告警）；打到 1GB 说明用户开了太多或某个服务跑飞了，该出声。
- 早先写的「单服务 1GB」形同虚设（正常 vite 才 145MB，永远碰不到），已改。

#### 一条重要的口径区分

**宿主不替服务省内存。** 这一层的职责是「知道它、能停它、退出时收干净」，
不是「给它做内存治理」。要给服务做内存治理是**容器 / systemd 单元**的活
（`systemd` 的 `MemoryMax=`、cgroup v2 的 `memory.max`）——那是部署形态的选择，
不该塞进一个桌面应用的工具层。宿主这层最多做到：**看得见、报得出、停得掉**。

### 5.5 落地上的一条硬约束

> **受管服务必须由宿主进程直接 `std::process::Command` spawn，且不得经由 `run_python` / 任何已设 rlimit 的子进程。**

这条是 §5.2 全部结论的落点，也是验收项（§15 有对应回归测试）。

---

## 6. 架构总览

```
                     ┌──────────────────────────────────────────┐
   模型 / Skill DSL  │            bot/dispatch.rs               │
   ─────────────────▶│  钩子 → 规则闸 → 子agent白名单 → JSON闸   │
                     │            │                             │
                     │            ▼                             │
                     │   bot/registry.rs TOOLS_TABLE            │
                     │   ┌────────────┬────────────┬──────────┐  │
                     │   │start_service│stop_service│list_services│
                     │   └──────┬─────┴─────┬──────┴──────────┘  │
                     │          │           │                    │
                     │          ▼           ▼                    │
                     │      ┌───────────────────────────┐       │
                     │      │   bot/services.rs（新）     │       │
                     │      │  登记表 / 状态机 / 停止阶梯  │       │
                     │      │  / 子孙回收 / 输出落盘      │       │
                     │      └───────────┬───────────────┘       │
                     │                  │                       │
                     │          AppState.services                │
                     │                  │                       │
                     │                  ▼                       │
                     │        std::process::Command             │
                     │        （宿主直接 spawn，不经 python）      │
                     └──────────────────────────────────────────┘

   应用退出：lib.rs::cleanup_on_exit_with
        ├─ stop_all_executions → api_stop → skill_terminate_all → drain 2s
        ├─ mark_exiting() → kill_all_py_children
        ├─ services::stop_all(app)          ← 本方案新增
        └─ mcp::manager::shutdown_all(app)
```

**新增模块**：`src-tauri/src/bot/services.rs`（单文件）。
单文件而非拆 `mod.rs + proc.rs`：仓库现有风格容忍大单文件（`py/runtime.rs` 1235 行、
`bot/registry.rs` 3642 行），且本方案只有一个调用方，拆开是预支结构。
预计 500–600 行，届时按"状态机 / 进程操作"再拆。

---

## 7. 工具形状

三个工具，全部进 `TOOLS_TABLE`（`bot/registry.rs:1776+` 追加表尾）。
`BUILTIN_TOOL_COUNT` 58 → **61**。

### 7.1 `start_service`

```jsonc
{
  "type": "function",
  "function": {
    "name": "start_service",
    "description": "启动一个受管长驻进程（node/deno/python 等白名单启动器），进程归宿主托管：活过本次工具调用，可查可停，宿主退出时自动收干净。适合 node MCP server、dev server、watcher 这类需要持续运行的东西；一次性脚本用 run_python。启动器必须在白名单内（npx/bunx/uvx/pipx/node/deno/python/python3/docker/podman），不传 shell 字符串。默认需用户在设置页开启「允许机器人启动本地服务」，授权模式为 yolo 时免开关。",
    "parameters": {
      "type": "object",
      "properties": {
        "command":   { "type": "string", "description": "启动器名，必须在白名单内（npx/bunx/uvx/pipx/node/deno/python/python3/docker/podman）" },
        "args":      { "type": "array", "items": { "type": "string" }, "description": "参数列表，逐个传入，不做 shell 解析" },
        "cwd":       { "type": "string", "description": "工作目录（绝对路径），受文件授权闸约束；省略则用宿主当前目录" },
        "name":      { "type": "string", "description": "用途标签，便于人识别，如「前端 dev server」；可省" },
        "readyPattern": { "type": "string", "description": "就绪判据：stdout 出现该正则才算启动成功。省略=进程起来 200ms 后仍活着即算成功" },
        "readyTimeoutSecs": { "type": "integer", "description": "等待就绪的超时秒数，默认 30，上限 120" }
      },
      "required": ["command"]
    }
  }
}
```

**返回**（紧凑 JSON 字符串，便于 Skill DSL 的 `${stepN.serviceId}` 取值）：

```json
{"serviceId":"svc_7f3a91c2","pid":48123,"command":"node","args":["server.js"],"cwd":"/Users/x/proj","state":"running","ready":true,"startedAt":1792576000000}
```

### 7.2 `stop_service`

```jsonc
{
  "name": "stop_service",
  "description": "停止一个受管服务。走降级信号阶梯：SIGINT → 100ms → SIGTERM → 400ms → SIGKILL，并递归回收脱离进程组的子孙进程。幂等：已停止的服务再停一次返回 stopped=false，不报错。",
  "parameters": {
    "type": "object",
    "properties": {
      "serviceId": { "type": "string", "description": "start_service 返回的服务 id" }
    },
    "required": ["serviceId"]
  }
}
```

**返回**：`{"serviceId":"svc_7f3a91c2","stopped":true,"method":"SIGTERM","descendantsKilled":2,"exitCode":0,"elapsedMs":118}`

### 7.3 `list_services`

```jsonc
{
  "name": "list_services",
  "description": "列出宿主托管的受管服务：id / 名称 / 命令 / 状态 / pid / 已运行秒数 / 退出码 / 最近输出尾部。用于确认服务是否还活着、拿服务 id 去 stop_service。",
  "parameters": { "type": "object", "properties": {} }
}
```

**返回**：`{"services":[{"serviceId":"...","name":"...","command":"node","state":"running","pid":48123,"uptimeSecs":132,"exitCode":null,"stdoutTail":["ready on http://127.0.0.1:5173"],"stderrTail":[]}]}`

### 7.4 ToolDef 登记项

| 工具 | `mutating` | `claims_patterns` | `max_output_bytes` |
|---|---|---|---|
| `start_service` | `true` | `&["已启动","已启动服务","服务已启动","已在后台启动"]` | 8192 |
| `stop_service` | `true` | `&["已停止","已停止服务","服务已停止","已关闭服务"]` | 8192 |
| `list_services` | `false` | `&[]` | 8192 |

`mutating: true` 的理由：起/停进程是有外部副作用的动作，谎报守卫要能识别
"我启动了服务"但本次查询里查不到 `start_service` 成功记录的情形。

### 7.5 Skill DSL 集成（已实测通过）

`start_service` 返回**纯 JSON 文本** → 调度器 `scheduler.rs:548` 的
`parsed: serde_json::from_str(&text).ok()` 能解析 → 变量替换
`vars.rs:29-30` 的 `VAR_NESTED_BY_INDEX` 正则匹配单段 key（非 `result`/`id`）→
`vars.rs:184-198` 的 `resolve_nested_path` 走 `parsed["serviceId"]`。

**实测**（临时探针跑 `cargo test --lib`；上游 step1 结果 =
`{"serviceId":"svc_7f3a91c2","pid":48123,"port":5173,"state":"running","ready":true}`）：

```
# 探针 1：占位符在 JSON 字符串位
输入 {"serviceId":"${step1.serviceId}","port":"${step1.port}","missing":"${step1.nope}"}
PROBE_OUT={"serviceId":"svc_7f3a91c2","port":"5173","missing":"${step1.nope}"}

# 探针 2：占位符在非字符串位
输入 {"port":${step1.port},"pid":${step1.pid},"ready":${step1.ready},"id":"${step1.serviceId}"}
PROBE2_OUT={"port":5173,"pid":48123,"ready":true,"id":"svc_7f3a91c2"}
PROBE2_PORT_IS_NUMBER=true   PROBE2_PID_IS_NUMBER=true   PROBE2_READY_IS_BOOL=true
```

即：

- 单段 key `serviceId` 能取到 ✓
- **占位符位置决定落型**（两次探针对比出来的）：
  - 在 **JSON 字符串位**（`"port":"${step1.port}"`）→ 落为**字符串** `"5173"`；
  - 在 **非字符串位**（`"port":${step1.port}`）→ 落为**真正的 JSON 数字** `5173`（探针 2 断言 `is_number() == true`），
    布尔同理落为 `true`（`is_boolean() == true`）。
  - 机制：`resolve_nested_path` 对非 String 终值走 `serde_json::Value::to_string()`（`vars.rs:193-197`），
    所以**拿到的永远是字符串形态**；它在非字符串位插入时是**裸插**，于是 `"5173"` 这个字符串的
    字面量恰好又是合法 JSON 数字。**能用，但要知道这件事**。
- **不存在的 key 保留占位符**（可诊断，不静默变空串）✓ —— 但残留的 `${...}` 会**原样传给下游工具**，
  下游看到的是字面量 `${step1.nope}` 而不是"缺参数"（§18 第 13 条）。

两个探针用完都已撤，`vars.rs` 回到原状。

所以 SKILL.md 里可以这样写（**端口放非字符串位**，才能拿到数字型）：

```markdown
## Step 1
- tool: start_service
  args: {"command":"node","args":["mcp-server.js"],"cwd":"/proj"}

## Step 2
- tool: fetch_url
  args: {"url":"http://127.0.0.1:${step1.port}"}
```

`${step1.serviceId}` 同样可取到（`resolve_nested_path` 对单段非 result/id 的 key 走 object 取值）。

> 注意：变量替换**只发生在 DSL 调度器**（`scheduler.rs:440`）。主 agent 的裸工具调用不走这层，
> 模型得自己从上一次工具返回里读 id。

### 7.6 四处基线断言必须同步（否则单测红）

`bot/registry.rs` 里**四处**手写的全量名单锁着内置工具集。加工具只改 `TOOLS_TABLE` 会红：

| 位置 | 断言 | 新增工具怎么改 |
|---|---|---|
| `registry.rs:1772` | `BUILTIN_TOOL_COUNT`（有 `tools_table_count_matches_constant` 单测守着） | 58 → **61** |
| `registry.rs:2795` | `let expected: HashSet<String>` —— 工具名**集合**（断言在 `:2856`） | 三个名字加进集合 |
| `registry.rs:2868` | `let expected_order: Vec<String>` —— 工具名**顺序**（断言在 `:2929`） | 按 `TOOLS_TABLE` 实际追加位置插入 |
| `registry.rs:2935` | `let expected: HashSet<&str>` —— **mutating 工具基线**（`mutating_tools_matches_baseline`） | `start_service` / `stop_service` 是 mutating → 加进去；`list_services` 不是 → 不加 |
| `registry.rs:3079` | `tools_json_matches_baseline` —— **主可见工具数**（`base_arr.len() + N`） | N 从 28 → **31**（+3 = 受管服务三工具）；断言消息里的算式也要跟着改，否则读的人对不上账 |

> **第四处是实施时才发现的**：前三条都在 `registry.rs` 的同一段测试里，第四条
> （`tools_json_matches_baseline`）隔得远、且它锁的是**数量算式**而不是名字列表，
> 读前三处时很容易以为「名字都加进去就完了」。它比的是 `tests/fixtures/tools_baseline.json`
> 里那 28 个核心工具 + 尾部增量，**新增工具必须把增量数一起改**。
>
> 教训：加内置工具时，基线断言不止「名字列表」这一类，**数量算式**也是一处。
> 实施时按「跑全量门禁，看哪个红」逐个补，比照着文档清单猜要可靠。

`expected_order` 锁的是**顺序**（"基线只比前缀"那种追加法在表尾加才安全），
所以三个新工具追加在 `TOOLS_TABLE` **表尾**，`expected_order` 同步在末尾追加。

**关于 feature 组合（实施前已查实）**：

- `TOOLS_TABLE` 区间（`registry.rs:1776-2368`）**没有任何 `#[cfg(...)]` 条目**；
  整个 `registry.rs` 里的 `cfg` 只有 4 处 `#[cfg(test)]`（测试模块）。
- `src-tauri/Cargo.toml` **没有 `[features]` 段**——项目根本没有 cargo feature 开关。

→ 结论：**今天的 `BUILTIN_TOOL_COUNT` 断言不是 cfg 敏感的**，加三个工具不会因为编译目标不同而对不上。
但这是个**易碎前提**：将来若引入 cargo feature 并用它 gate 某个工具条目，
`tools_table_count_matches_constant` 会在某些编译配置下红（count 与表长度不等）。

**实施时要做的一件事**：在 `TOOLS_TABLE` 附近加一条**编译期/单测期守卫**——
扫 `TOOLS_TABLE` 是否含 cfg-gated 条目（或直接断言 `BUILTIN_TOOL_COUNT` 在所有 feature 组合下相等），
让"有人加了 feature-gated 工具"这件事**当场红**，而不是等到某天某个 feature 组合下才炸。
比"记住这件事"可靠。

### 7.7 四处 profile 列表不动

**主 agent 的工具来源是 `TOOLS_TABLE` 全量**（`tools_json()` 走全表），不经过下面这四个常量。
四个 profile 列表**只用于子 agent 的收窄**——schema 层按 `profile ∩ per-onion allowed` 过滤，
dispatch 层再用 `ctx.tools` 兜一道（`bot/dispatch.rs:431-464`）。

所以"三个新工具不在任何 profile 里"的准确含义是：**子 agent 拿不到起服务的权限**，
而**主 agent 照常看得见这三个工具**。`GENERAL_TOOLS` 同理不动——它不是"通用工具全集"，
是"通用子 agent profile 的允许清单"。

| 常量 | 位置 | 用途 | 处置 |
|---|---|---|---|
| `SUBAGENT_ONLY_TOOLS` | `registry.rs:2368` | 仅子 agent 可见的工具 | 不动 |
| `RESEARCH_TOOLS` | `registry.rs:2378` | research 子 agent profile | 不动 |
| `CODER_TOOLS` | `registry.rs:2388` | coder 子 agent profile | 不动 |
| `GENERAL_TOOLS` | `registry.rs:2401` | general 子 agent profile | 不动 |

→ 子 agent 拿不到起服务的权限（`bot/dispatch.rs:431-464` 的白名单闸会拒）。要放开再显式加进对应 profile。

---

## 8. 登记表

### 8.1 位置

`AppState`（`app_state.rs:82-113`）新增字段，与 `skill_runs` 同款形态：

```rust
pub(crate) struct AppState {
    // ...既有 12 张表...
    /// 受管长驻服务表：serviceId → ManagedService
    pub(crate) services: Arc<Mutex<HashMap<String, ManagedService>>>,
}
```

访问器（`app_state.rs:145-149` 同款）：

```rust
pub(crate) fn services<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Arc<Mutex<HashMap<String, crate::bot::services::ManagedService>>> {
    ext(app).services.clone()
}
```

放 `AppState` 而不是全局 `static` 的理由：`app_state.rs` 模块头已声明它是"执行期全局状态单一入口"；
`skill_runs` 是同类先例；测试隔离靠 `app.manage(AppState::default())` 注入自己的实例。

### 8.2 数据结构

```rust
/// 受管服务状态。终态 Exited / Failed 不再变更。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceState {
    /// 进程已 spawn，等就绪判据（readyPattern）或短存活确认
    Starting,
    /// 已确认存活
    Running,
    /// 停止流程进行中
    Stopping,
    /// 进程已退出（正常结束或被杀），exit_code 有值
    Exited,
    /// 根本没起来（spawn 失败 / 闸拒绝 / 命令不在白名单）
    Failed,
}

/// 登记表条目。`Child` 句柄不在这张表里——spawn 后立刻移交给 watcher 线程，
/// 停止只需要 pid（发信号），不需要 Child。
pub(crate) struct ManagedService {
    pub(crate) id: String,              // "svc_<8位hex>"
    pub(crate) name: String,            // 用途标签（可空串）
    pub(crate) command: String,         // 启动器名
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) pid: u32,
    pub(crate) pgid: i32,               // unix 进程组 id（= pid，因 process_group(0)）
    pub(crate) state: ServiceState,
    pub(crate) started_at_ms: i64,
    pub(crate) exited_at_ms: Option<i64>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stop_reason: Option<String>,
    /// stdout/stderr 落盘路径（`O_APPEND`，宿主可随时 truncate 不影响子进程续写）
    pub(crate) stdout_path: PathBuf,
    pub(crate) stderr_path: PathBuf,
    /// 归属会话（仅记录，不触发自动回收——见 §13.4）
    pub(crate) owner_session: Option<String>,
    /// 就绪瞬间的 stdout 尾部快照（最近 2KB）。**不可截断**——日志文件会被 §11.3
    /// 的巡检截断，但启动横幅 / 监听地址这类关键信息不能跟着丢（见 §11.5）。
    pub(crate) ready_snapshot: Option<String>,
}
```

**为什么 `Child` 不进登记表**：Rust 的 `Child` 不能 clone，塞进 `Mutex<HashMap<..>>` 会让
"读状态"和"等进程"共用一把锁。设计上：spawn 成功后 `Child` 立刻 `move` 进 watcher 线程
（`thread::spawn(move || { let status = child.wait(); ... })`），登记表只留 pid。

### 8.3 进程组

unix 下 spawn 时 `cmd.process_group(0)`（与 `py/runtime.rs:789` 同款），
子进程自成进程组，pgid == pid。这样"杀组"和"杀进程"是同一件事的两个 id 视图，
且 `setsid` 逃逸的子孙会**脱离**这个组——所以还需要 ppid 树回收（§10.3）。

---

## 9. 生命周期状态机

```
                    start_service
                         │
                         ▼
                  ┌─────────────┐
                  │  Starting   │  spawn 成功但还没确认就绪
                  └──────┬──────┘
       readyPattern 命中  │  200ms 后仍存活（无 readyPattern）
       或短存活确认       │
                         ▼
                  ┌─────────────┐        ┌──────────┐
                  │   Running   │───────▶│ Exited   │ ◀── watcher: child.wait() 返回
                  └──────┬──────┘        └──────────┘
                         │                    ▲
     stop_service / 退出  │                    │
                         ▼                    │
                  ┌─────────────┐             │
                  │  Stopping   │─────────────┘
                  └─────────────┘   阶梯结束（SIGKILL 后 wait）
```

### 9.1 状态迁移表

| 从 | 到 | 触发 | 副作用 |
|---|---|---|---|
| — | `Failed` | 命令不在白名单 / 权限闸拒绝 / spawn 失败 | 审计 `service.start_failed`；工具返回 error |
| — | `Starting` | spawn 成功 | 建 pid/pgid；起 watcher 线程 |
| `Starting` | `Running` | readyPattern 命中（≤ readyTimeoutSecs） | 审计 `service.ready` |
| `Starting` | `Running` | 无 readyPattern 且 200ms 后仍存活 | 同上 |
| `Starting` | `Exited` | 就绪等待期内进程退出 | 审计 `service.exited`；工具返回 error + stderr 尾部 |
| `Starting` | `Exited` | 就绪超时 | **主动停止该服务**（阶梯），工具返回 error「就绪超时」 |
| `Running` | `Stopping` | `stop_service` / 应用退出 | 审计 `service.stop_requested` |
| `Stopping` | `Exited` | 阶梯走完 | 审计 `service.stopped`（带 method / descendantsKilled / elapsed） |
| `Running` | `Exited` | 进程自己退出 | watcher 写 exit_code；审计 `service.exited` |
| `Exited` / `Failed` | — | 终态 | 登记项**保留**在表里（供 `list_services` 查历史），但从"活跃服务"计数中排除 |

### 9.2 终态保留

`Exited` / `Failed` 的条目**留在登记表**里（不立刻删），理由：
`list_services` 要能回答"我刚才起的那个服务怎么死了"——这是最常见的一问。
活跃计数（并发上限 4）只算 `Starting` / `Running` / `Stopping`。

登记表清理：应用退出时整体清空（随 `AppState` 一起没了）。会话内不清理——
条目数量上限 = 并发上限 + 本次会话内起过的服务数，起得多也不至于失控（每条几百字节）。

### 9.3 僵尸处理（必须做）

异步运行的子进程，父进程不 `wait()` 就会变僵尸。
`py/runtime.rs` 的 `PY_CHILDREN` + 同步 `try_wait` 轮询模式**不适用**这里（那是作业制的同步等待）。

**做法**：每个服务 spawn 后立刻起一个 watcher 线程：

```rust
let svc_id = id.clone();
let table = services_table.clone();
std::thread::spawn(move || {
    let status = child.wait();          // 阻塞直到进程退出
    // 写回 exit_code / exited_at / state = Exited
    // 审计 service.exited
});
```

线程在进程退出后自然结束，不泄漏。`AppState` 里的条目用 `Arc` 共享，watcher 线程只写自己那条。

---

## 10. 停止信号阶梯与子孙回收

### 10.1 阶梯（unix）

抄 Claude Code 官方 mcp 文档的毫秒数：

```
SIGINT ──▶ 等 100ms ──▶ 仍存活? ──▶ SIGTERM ──▶ 等 400ms ──▶ 仍存活? ──▶ SIGKILL ──▶ wait()
```

- 每一级之间用 `kill -0 <pid>` 判存活（发信号失败 = 已死）。
- `SIGINT` 给的是**优雅退出**机会：node 收到 SIGINT 会跑 exit handler（关连接、刷盘）。
- `SIGKILL` 后必须 `wait()` 收回（watcher 线程会自然 wait 到，主流程不重复 wait）。
- 整条阶梯的最坏耗时 = 100 + 400 + 信号投递 ≈ **500ms + ε**（不含进程自己不退出的额外时间——不，设了上限）。

### 10.2 阶梯（windows）

抄同一份文档的 Windows 分支：先关 stdin → 等 300ms → `taskkill /PID <pid> /T /F`。

关 stdin 对 stdio 型服务有效；本方案的服务 stdin 是 `Stdio::null()`，所以这一级实际是
"逻辑等待窗口"，真正杀的是 `taskkill /T /F`（`/T` = 杀进程树）。`/T` 已覆盖子孙，
但 ppid 树补杀仍做（`/T` 对已脱离的孙进程同样无能为力）。

### 10.3 子孙回收（本方案相对 `run_python` 的增强）

`run_python` 只杀进程组（`py/runtime.rs:205-229`）。`setsid` 逃逸的子孙**不在组里**，
会被漏掉——`DEVLOG.md:4150` 把这条记成"已知 trade-off 不修"。

**服务层不能接受这条 trade-off**：需求就是"宿主知道它、能停它、退出时收干净"。
一个逃逸的 node 子进程 = 宿主管不到的孤儿。

#### 快照与收集（纯函数，不碰登记表）

整个进程树操作拆成**只依赖 pid 空间**的纯函数，不读登记表、不认识 `ManagedService`：

```rust
/// 进程快照的一行
pub(crate) struct ProcSnapshot {
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    /// 进程启动标识。Linux = `/proc/<pid>/stat` 第 22 字段（starttime，clock ticks
    /// since boot）；macOS = `ps -o lstart=` 的启动时间串。用于识别 **PID 复用**。
    pub(crate) starttime: String,
    /// 常驻内存（字节）。Linux = `/proc/<pid>/stat` 第 24 字段（页数 × 页大小）；
    /// macOS = `ps -o rss=`（KB）。资源观测用。
    pub(crate) rss_bytes: u64,
    /// 累计 CPU 时间（秒）。Linux = (utime+stime)/CLK_TCK；macOS = `ps -o time=`。
    /// 两次采样的差值 ÷ 时间差 = 占用率。
    pub(crate) cpu_seconds: f64,
}

/// 拍一张全机进程快照。只读系统进程表，不碰本模块登记表。
pub(crate) fn take_snapshot() -> Vec<ProcSnapshot>;

/// 从快照里收集 root 的全部后代（纯计算，无 IO）。
pub(crate) fn descendants(snap: &[ProcSnapshot], root: u32) -> HashSet<(u32, String)>;

/// 增量合并两次快照的差集：返回**新增**的后代，并剔除 PID 复用者。
pub(crate) fn merge_descendants(
    prev: &HashSet<(u32, String)>,
    cur: &HashSet<(u32, String)>,
) -> Vec<u32>;
```

> **纯函数是刻意的**：`collect_and_kill_tree(root_pid)` 只依赖 pid，不碰登记表。
> 将来 MCP stdio 要接同一套停止阶梯时，这块**直接上提**成公共模块即可，
> 不需要为了"以后能复用"而现在就提前抽象（§14.1）。

#### 增量快照：每级信号前拍一张

**不是"阶梯开始前拍一次 + 结束后拍一次"**——那个设计有两个洞：

1. 两次快照之间的那 500ms 里新 fork 出来的子孙抓不到；
2. "结束后再拍"时父进程可能已死，逃逸子孙已被 reparent 到 `launchd`（ppid=1），
   父子链已经断了，**再也认不出它是谁的后代**。

**改成：阶梯每一级发信号之前拍一张快照，增量收集，最后统一杀。**

```
第 1 级：拍快照 S1 → descendants(S1) → 待杀集合 D = D ∪ S1 的后代
         发 SIGINT → 等 100ms
第 2 级：拍快照 S2 → descendants(S2) → D = D ∪ (S2 的后代 − 已有)
         发 SIGTERM → 等 400ms
第 3 级：拍快照 S3 → descendants(S3) → D = D ∪ (S3 的后代 − 已有)
         ├─ 杀进程组：kill -9 -<pgid>
         └─ 统一补杀：对 D 里每个 pid 逐个 kill -9
```

三条要点：

- **只收集，不边收边杀**：收集阶段只往 `D` 里加 pid，杀全部推迟到最后一步。
  边收边杀会打断正在进行的快照（进程在快照中途消失 → 父子链残缺 → 漏收），
  而且中途的 kill 可能让目标提前逃逸。
- **最后一张快照紧邻 SIGKILL**：逃逸窗口被压到最小（最后一次快照到 SIGKILL 之间只隔一次 kill 调用）。
- **集合是并集**：D 里既有第一轮就抓到的逃逸子孙，也有最后一刻才 fork 出来的。

#### PID 复用必须防

pid 是可复用的。停止一个服务要花 500ms+，这期间系统完全可能把某个已死进程的 pid
分配给一个**毫不相干的新进程**。若只按 pid 补杀，就会**误杀别人的进程**。

**处置**：快照每一行都带 `starttime`（进程启动标识）。合并时，
同一个 pid 在新旧快照里 `starttime` 不一致 → 说明 pid 被复用了 → **剔除该 pid，不杀**。

宁可漏杀（那个进程已经不是我们的子孙了）也不能误杀（可能杀掉用户正在跑的东西）。
剔除动作记审计 `service.pid_reuse_skipped`。

#### 平台差异

- **Linux**：`/proc/<pid>/stat` 第 22 字段 = `starttime`（clock ticks since boot，精度到 tick）。
- **macOS**：无 `/proc`。用 `ps -o pid= -o ppid= -o lstart= -ax` 取 `lstart` 启动时间串
  （精度到秒——**够用**：pid 复用不可能发生在同一秒内）。
- **Windows**：用 `taskkill /T`（系统自带进程树），不做 ppid 树解析，也不存在 pid 复用问题
  （Windows 的 pid 复用有不同语义，交给系统命令处理）。

#### 整组杀的护栏（实施时补上）

阶梯最后一步要 `kill -9 -<pgid>`，但组 id 就是组长的 pid，而 pid 会被回收、复用：

- 组长被 watcher 收尸后，pid **立刻被释放**（这是 pid 复用的前提）；
- 之后这个号可能被系统分给**毫不相干的新进程**，而新进程若自成组长，
  `killpg(组长 pid)` 就会打到**别人的组**上。

所以整组杀不能只看"pid 还在不在"——`kill(pid, 0)` 对"我们的组长"和"号被借走了"
两种情况都返回成功。**判据是启动标识**：快照里组长的启动标识与停止开始时记下的
一致，才说明"还是我们的组长"，才允许整组杀；不一致或组长已不在快照里（走了且被回收）
就**只逐个补杀**已核验过的后代（宁可漏杀也不误杀，见上面的 PID 复用一节）。

同理，`kill(0, sig)` / `killpg(0, sig)` 的语义是"给调用方的整个进程组发信号"，
一旦 pid 拿不到（登记表占位条目还没填 pid）就会变成"宿主给自己发信号"——
这两个调用必须显式拒掉 pid ≤ 0。

#### 与 `DEVLOG.md:4150` 的关系

那条记录的是 `run_python` 沙箱里的孙进程逃逸。
本方案**不改 `run_python`**，只在新层做回收。口径不同是因为约束不同：
沙箱作业是短命的、逃逸的孙进程随之无害；长驻服务是用户资产，逃逸 = 宿主失去管控。
这个差异记在仓库 `DEVLOG.md`（2026-10-21 本轮条目）。

### 10.4 停止是幂等的

`stop_service` 对已 `Exited` 的服务 → 返回 `{"stopped":false,"reason":"already exited"}`，
**不报错**。模型重试、空跑、并发 stop 都不该炸。

---

## 11. 输出落盘与上限

### 11.1 为什么落文件而不是管道

`run_python` 死法的第 2 条（`py/runtime.rs:941-946`）是：孙进程占着 stdout 管道不放 →
2s drain 收不齐 → **连坐杀整组**。受管服务恰恰是"会一直占着管道"的东西。

**所以受管服务的 stdout/stderr 直接重定向到文件，不占任何管道。**

### 11.2 打开方式：`O_APPEND`

```rust
let f = OpenOptions::new().create(true).append(true).open(path)?;
cmd.stdout(Stdio::from(f)).stderr(Stdio::from(other_f));
```

用 `append(true)`（`O_APPEND`）而不是普通写，是为了让宿主的截断操作安全：
- 宿主把文件截到 N 字节后，子进程下一次 `write` 会写到**当前文件末尾**（`O_APPEND` 语义），
  不会在旧偏移处写出稀疏空洞。
- 普通写模式（无 `O_APPEND`）下，截断后子进程仍按原偏移写 → 文件变成"前面一段空洞 + 后面正常"，
  体积统计和 `tail` 都会失真。

**附带好处**：子进程写 stdout **不会**因为宿主读得慢而阻塞（没有管道缓冲区），
也就不会"管道满 → 子进程卡在 write → 宿主以为它死了"。

### 11.3 输出上限（硬）

- 默认 **8MB / 流**（stdout、stderr 各算）。
- 宿主巡检（默认 30s 一轮，在 `list_services` / `start_service` / `stop_service` 时也顺带做一次）发现超限：

  ```rust
  // 1) 读末尾 N/2 字节（独立只读 fd）
  let tail = read_tail(&path, cap / 2)?;
  // 2) 截断到 0
  truncate(&path, 0)?;
  // 3) 关键：写回也必须用 append 模式打开
  let mut w = OpenOptions::new().append(true).open(&path)?;
  w.write_all(&tail)?;
  ```

  4. 审计 `service.output_truncated`（带 `bytes` / `cap`）。

- 效果：文件大小稳定在 8MB 上下，永远只保留**最近的**输出（对诊断来说这正是要的）。

#### 写回 fd 也必须是 `O_APPEND`（易漏点）

子进程持有的写 fd 是 `O_APPEND`（`O_APPEND` 的语义是"每次写都落到**当前文件末尾**"）。
宿主如果用**普通写模式**打开同一个文件去写回尾部，写位置是宿主自己 fd 的偏移
（新打开的 fd 偏移 = 0）→ 会**从文件开头覆盖**，把刚写进去的尾部、以及子进程并发写的内容一起冲掉。

**所以截断重写的写 fd 同样以 append 模式打开**：截断后文件长度为 0，append 写落到偏移 0，
尾部内容正好写在开头，位置正确；子进程此后的 append 写继续跟在后面。

三个 fd 的开法一览：

| fd | 打开方式 | 用途 |
|---|---|---|
| 子进程 stdout/stderr | `create + append` | 唯一真相源，子进程持续写 |
| 宿主读 fd | 只读 | 取尾部 |
| 宿主写 fd（截断重写） | **`append`** | 写回尾部 —— 必须是 append，不是普通写 |

#### 截断窗口内的输出可能丢失（已知代价）

`set_len(0)` 与写回之间有一个几毫秒的窗口。子进程在这窗口里的写会：

- 落在 `set_len(0)` **之后**、宿主写回**之前** → 它的内容被排在尾部内容**前面**（顺序错乱），
  文件长度多出这一段；
- 落在宿主**读尾部之后**、`set_len(0)` **之前** → 这段输出**被直接抹掉**（真丢失）。

两种都是"丢几毫秒的日志"，不是数据损坏（文件不会出现空洞，因为全程 append）。
**接受这个代价**，理由：日志尾部本来就是"最近发生了什么"的近似，丢几毫秒不影响诊断；
而 §11.5 的就绪快照已经把**关键信息**（启动横幅、监听地址）单独固化下来了。
代价记进 §18。

### 11.4 输出展示

`list_services` / `stop_service` 的返回里带**最近 20 行**（stdout / stderr 各一份）。
实现：读文件末尾 ~8KB，按行切，取最后 20 行。**不维护内存环形缓冲**——
文件是唯一真相源，少一份状态就少一处会漂移的地方。

### 11.5 就绪快照（不可截断）

**问题**：服务启动时打的横幅（`ready on http://127.0.0.1:5173`）是**唯一**能拿到监听地址的地方。
但日志文件会被 §11.3 的巡检截断成"最近 8MB"——一个跑了几小时的 dev server，
它的启动横幅早就被后续输出冲掉了。届时 `list_services` 给不出端口，诊断缺口。

**做法**：**就绪判定通过的那一刻**，立刻把 stdout 文件**末尾最近 2KB**拷进
`ManagedService.ready_snapshot`——一个**独立于日志文件、不参与截断**的内存副本。

- 落点：内存字段 `ready_snapshot`（每服务 ≤ 2KB，4 个服务 = 8KB，可忽略）。
- 同时写进 `service.ready` 审计 KV（`stdout_head` / `stdout_tail` 两段，各 1KB）。
- `list_services` / `stop_service` 的返回里带上它 → 模型能直接看到监听地址。

**成本极小，缺口实在**：日志负责"最近发生了什么"，就绪快照负责"它是怎么起来的"。
两者职责不重叠，也不能互相替代。

---

## 12. 权限闸（五道）

| # | 闸 | 判据 | 拒绝时的表现 |
|---|---|---|---|
| 1 | **设置页独立开关** | `flags_dir/services-enabled.flag` 存在（与 `py-enabled.flag` 同款，`py/commands.rs:10-34`） | 拒绝，`ToolResult::denied` |
| 2 | **yolo 免开关** | `crate::bot::perm_mode(app) == PermMode::Yolo`（口径同 `py/document.rs:2003-2006`） | 放行 |
| 3 | **启动器白名单** | `STDIO_LAUNCH_ALLOWLIST.contains(&command)`（`bot/mcp/config.rs:49-51`） | 拒绝，**不 spawn** |
| 4 | **路径授权闸（cwd）** | `bot_fs::resolve_with_perm(app, "start_service", cwd, interactive, session_id)`（`bot_fs.rs:317`） | 白名单外按 `perm_mode` 弹窗确认；后台无人应答 → 拒绝 |
| 5 | **per-tool 规则表** | `bot/dispatch.rs:401-407` 的 `tool_rule_gate`（设置页授权卡可 deny/ask/allow） | 自动生效，无需新代码 |

### 12.1 白名单复用与逐臂构造

```rust
// 单一事实源：bot/mcp/config.rs:49-51
if !STDIO_LAUNCH_ALLOWLIST.contains(&command) {
    return Err(format!("命令「{command}」不在启动器白名单内（允许：{}）",
        STDIO_LAUNCH_ALLOWLIST.join(" / ")));
}
// 逐臂字面量构造：源码层面不存在「运行时变量 → 进程名」路径
let mut cmd = match command {
    "npx"    => Command::new("npx"),
    "bunx"   => Command::new("bunx"),
    "uvx"    => Command::new("uvx"),
    "pipx"   => Command::new("pipx"),
    "node"   => Command::new("node"),
    "deno"   => Command::new("deno"),
    "python" => Command::new("python"),
    "python3"=> Command::new("python3"),
    "docker" => Command::new("docker"),
    "podman" => Command::new("podman"),
    _ => return Err(/* 与 contains 侧同文案 */),
};
```

与 `bot/mcp/manager.rs:209-235` **完全同款**（含双清单 parity 单测的做法）。
不新增白名单、不新增依赖、不走 shell、不做字符串拼接。

### 12.2 为什么必须有独立开关

`run_python` 有开关（"允许机器人执行 Python"），因为那是危险能力。
**启动一个长驻本地进程同样是危险能力**——它可以监听端口、可以常驻、可以自启动后脱离管控。
默认关闭是唯一安全的默认。

### 12.3 子 agent 闸

新工具**不进**任何 subagent profile 的 allowed tools 列表
（`SUBAGENT_ONLY_TOOLS` / `RESEARCH_TOOLS` / `CODER_TOOLS` / `GENERAL_TOOLS` 四处，见 §7.7）。
→ 子 agent 天然不能起服务（`bot/dispatch.rs:431-464` 的白名单闸会拒）。
需要时再显式加进 profile。

### 12.4 env 政策

**不开放自定义 env。** 服务继承宿主环境（用户在终端里 `npm run dev` 也是继承环境，这是常规语义）。
理由：自定义 env 是密钥注入面（MCP 那边 env 值已经按机密处理、存 keyring），
而本方案没有第二个使用方要求它。

**风险披露（必须让用户看见，不是"实现细节"）**：

> **开启本能力 = 信任「被启动的代码能读到宿主进程的全部环境变量」。**
> 宿主环境里通常有 API key、token、数据库口令（`~/.minimax/config.yaml` 的 provider key、
> `TAVILY_API_KEY`、`ANTHROPIC_API_KEY`、各种 `*_TOKEN`……）。
> 这些凭证**不在白名单里、不会被拦、也不会被记录**——因为我们根本不区分环境变量。
> `yolo` 档更是**没有二次确认**：一次调用就等于把这台机器上的凭证交出去了。

所以：

- 设置页开关的说明文字必须**逐字写明这条**，不能让用户以为"就是个起进程的功能"。
- 审计里记 argv（§12.5），但**不记环境变量**（记了就是泄密面）。
- 被启动的代码若是自己写的（`node ./my-server.js`）——它能读到宿主凭证这件事用户自己清楚。
  风险在于**模型指定的、用户没审过的代码**（`npx <某个包>` 就会执行第三方包的代码）。
  → 这是**白名单闸的真正价值所在**：`npx`/`uvx`/`pipx` 在白名单里（能装包、能跑任意第三方代码），
  但**绝对路径的第三方二进制不在白名单里**（`/tmp/whatever` 直接拒）。
  白名单把"能跑任意代码"这件事收敛到"包管理器的官方入口"，这已经是不小的收窄，
  但**它不是安全边界，是减少意外**。这一点要说清楚，不能让人以为开了开关就安全了。

### 12.5 argv 审计

`service.started` 审计记**完整 argv**（截断到 400 字符）——起什么命令、监听什么端口，
事后可追溯。argv 里的路径和端口不是敏感信息；若将来发现某天 argv 会带 token，
再加一个脱敏层。

---

## 13. 退出清理

### 13.1 挂载点

`lib.rs::cleanup_on_exit_with`（`lib.rs:222-248`）加一步：

```rust
bot_py::mark_exiting();
let py_killed = kill_py();
// 新增：受管服务统一收尾（走降级阶梯 + 子孙回收）
let services_stopped = bot::services::stop_all(app);
bot::mcp::manager::shutdown_all(app);
```

位置在 `kill_py` 之后、`mcp::shutdown_all` 之前：先收自己起的服务，再收 MCP。

### 13.2 退出时的停止流程

`stop_all(app)`：

1. 快照活跃服务（`Starting` / `Running` / `Stopping`）。
2. **并发**执行停止（每服务一个任务）——串行的话 4 个服务 × 500ms = 2s，退出体验差。
3. 总预算 **2s**（与既有 `EXIT_DRAIN_GRACE` 同量级，`lib.rs:201`）。超时 → 全部 SIGKILL，不无限等。
4. 删掉 stdout/stderr 日志文件与工作目录（`flags_dir` 同级的 temp 目录）。
5. 写 `app_exit_cleanup` 审计，**带上每个服务的 stderr 尾部摘要**（诊断不因删文件而丢）。
6. 清空登记表。

### 13.3 退出时不能做的事

- **不能**因为"服务还在跑"就跳过停止——那正是孤儿来源。
- **不能**在退出路径里 `wait()` 无限期——进程不配合就得能放弃（SIGKILL 后不再等）。

### 13.4 会话结束不自动回收（明确的取舍）

服务的生命周期终点只有三个：**显式 `stop_service`**、**应用退出**、**进程自己退出**。
会话结束**不**触发停止。

理由：wmessage 没有"会话结束"的单一事件源（`StopGuard` 是 `/stop` 置位，会话可以继续），
硬绑上去要么绑错时机、要么误杀跨会话复用的服务（用户开新会话继续用同一个 dev server 是合理诉求）。
模型可以在任务收尾时主动调 `stop_service`——这也是 `stop_service` 存在的意义。

> 这条不实现是有代价的：用户不调 `stop_service` 也不退出应用，服务会一直留着。
> 缓解手段是 §5.3 的 RSS/CPU 观测告警 + §11.3 的输出上限，两者都会在 `list_services` 里可见。

---

## 14. 与现有模块的边界（改动点清单）

| 文件 | 改什么 | 明确不改什么 |
|---|---|---|
| `src-tauri/src/bot/services.rs` | **新增**：登记表 / 状态机 / spawn / 停止阶梯 / 子孙回收 / 输出落盘 / 闸判定 | — |
| `src-tauri/src/bot/mod.rs` | 加 `pub mod services;` | — |
| `src-tauri/src/app_state.rs` | `AppState` 加 `services` 字段 + `services(app)` 访问器 | 其余 12 张表不动 |
| `src-tauri/src/bot/registry.rs` | `TOOLS_TABLE` 追加 3 条 + 3 个 `SCHEMA_*` 常量 + `BUILTIN_TOOL_COUNT` 58→61 + **四处基线断言同步**（见 §7.6）；`call_start_service` 算好权限档与路径闸再注入工具（见 §16.3） | 四处 profile 工具列表**不动**（子 agent 不给起服务的权限） |
| `src-tauri/src/bot/tools.rs` | 3 个 `tool_start_service` / `tool_stop_service` / `tool_list_services` 薄壳（工具本体不绑 Runtime，Wry 依赖注入进来——见 §16.3） | 既有 58 个工具不动 |
| `src-tauri/src/lib.rs` | `cleanup_on_exit_with` 加 `services::stop_all(app)` 一行 + 审计字段 | 退出顺序其余部分不动 |
| `src-tauri/src/py/commands.rs` | 加 `service_get_enabled` / `service_set_enabled`（flag 文件，与 `py-enabled.flag` 同款） | `py_get_enabled` 不动 |
| `src-tauri/src/py/runtime.rs` | **不改** | 全部 |
| `src-tauri/src/bot/mcp/manager.rs` | **不改** | 全部（含 `TokioChildProcess` Drop 兜底 kill） |
| `src-tauri/src/bot/dispatch.rs` | **不改** | 全部（闸自动生效） |
| `src-tauri/src/bot_skills/**` | **不改** | 全部（变量替换已支持，见 §7.5） |
| `src-tauri/src/bot_fs.rs` | **不改** | 全部（`resolve_with_perm` 复用） |
| `docs/rust-bot-architecture.md` | **实施时**补模块树条目 + `AppState` 表清单 | 本期不动（现行架构以现状为准，设计稿不冒充已落地） |

### 14.1 与 `bot/mcp/manager.rs` 的关系（为什么不合并）

MCP stdio server 也是"宿主 spawn 的长驻子进程"，看起来可以合并。**不合并**，三条理由：

1. **管道策略相反**：MCP 的 stdin/stdout 是 rmcp 的协议管道（必须接），
   受管服务**故意不用管道**（§11.1，正是为了绕开 drain 超时连坐杀组）。
2. **生命周期模型不同**：MCP 由 rmcp 的 `ClientService` 持有，靠 `TokioChildProcess` 的 Drop 兜底 kill，
   要换成降级阶梯就得动 rmcp 集成——风险高、收益低。
3. **上层语义不同**：MCP 有 initialize 握手 + 工具发现 + 挂载；受管服务是裸进程，用户自己连。

**可以共享的**：`STDIO_LAUNCH_ALLOWLIST`（已是单一事实源）、逐臂构造模式。
**停止阶梯 + 子孙回收已共享**（P2-② 落地）：§10.3 的进程树操作按纯函数设计
（`take_snapshot` / `descendants` / `diff_descendants` 只依赖 pid 空间，不碰登记表），
第二个调用方（MCP stdio 退出收尾）出现后整体上提为 **`bot/procs.rs`**：
`procs::stop_tree(root_pid, pgid, wait_exit)` 走同一套阶梯与子孙回收，
两个上层只差「怎么判进程退了」（服务层看登记表状态，MCP 侧看 `kill(pid, 0)`）。

MCP 侧接阶梯解决的是一个真问题：rmcp 的 `TokioChildProcess` Drop 只是
fire-and-forget 一个 SIGKILL——没有优雅退出窗口、不回收脱离进程组的子孙，
而且那个 kill 是 `tokio::spawn` 出去的，应用正在退出时它可能根本没来得及跑
（子进程留成宿主管不到的孤儿）。现在 MCP stdio 子进程自建进程组，
退出时先走阶梯停干净，再让 rmcp 收尾（此时已是死进程，Drop 的 kill 是空操作）。

### 14.2 与 `py/runtime.rs` 的关系（为什么不复用）

`py/runtime.rs` 是**同步阻塞作业执行器**：spawn → 读管道 → `try_wait` 轮询 → 超时杀组 → 删目录。
受管服务要的是**异步长驻**：spawn → 立刻返回 → watcher 线程等 → 登记表跟踪。
两者时序模型相反，复用 `run_python_at` 等于把它的同步轮询和超时语义一起拖过来。

**唯一的共用地盘是"进程组"这个概念**——两边都用 `process_group(0)`，但用途不同：
沙箱层用它是"能整组杀干净"，服务层用它是"能整组发信号 + 需要额外抓逃逸子孙"。

---

## 15. 审计事件

| 事件 | 级别 | KV |
|---|---|---|
| `service.started` | Info | `service_id`, `command`, `args_preview`(≤400), `cwd`, `pid` |
| `service.ready` | Info | `service_id`, `pid`, `elapsed_ms`, `stdout_head`(≤1KB), `stdout_tail`(≤1KB) |
| `service.start_failed` | Warn | `service_id`, `command`, `reason` |
| `service.exited` | Info | `service_id`, `pid`, `exit_code`, `uptime_ms` |
| `service.stop_requested` | Info | `service_id`, `reason`(`tool`/`app_exit`/`ready_timeout`) |
| `service.stopped` | Info | `service_id`, `method`(signal used), `elapsed_ms`, `descendants_killed` |
| `service.pid_reuse_skipped` | Warn | `service_id`, `pid`, `old_starttime`, `new_starttime` —— 该 pid 在快照之间被复用，**不杀** |
| `service.resource_warning` | Warn | `service_id`, `scope`(`service`/`total`), `rss_mb`, `cpu_pct`(仅有 CPU 告警时), `threshold` |
| `service.denied` | Warn | `command`, `reason`（开关关 / 白名单 / 路径闸） |
| `service.output_truncated` | Info | `service_id`, `stream`, `bytes`, `cap` |
| `service.resource_warning` | Warn | `service_id`, `rss_mb` / `cpu_pct`, `threshold` |
| `app_exit_cleanup` | Info | 既有字段 + **新增** `services_stopped` |

事件名沿用 `service.` 前缀（与 `tool.*` / `skill_*` / `perm.*` 风格一致）。

---

## 16. 测试计划

### 16.1 纯函数单测（`src-tauri/src/bot/services.rs` 内）

| 用例 | 断言 |
|---|---|
| 命令不在白名单 | 返回 Err，**不 spawn**（用 `command` 字段断言没有副作用） |
| 白名单逐臂与 `STDIO_LAUNCH_ALLOWLIST` **parity** | 两清单集合相等（照 `manager.rs` 的 parity 锁法） |
| ppid 树收集 | 给定假 `(pid, ppid)` 表，收集到的后代集合正确（含多层嵌套） |
| ppid 树收集不误伤 | 与 root **无祖先关系**的 pid 不进集合（不能把别人的进程杀了） |
| **增量快照合并** | 两次快照的差集 = 新增后代；已收集的不重复入集合 |
| **PID 复用剔除** | 同一 pid 在新旧快照里 `starttime` 不同 → **不进待杀集合**（宁可漏杀不可误杀） |
| **整组杀护栏** | 组长启动标识一致 → 允许整组杀；标识变了 / 组长不在快照里 / 从没记到标识 → **不允许**（`group_kill_allowed`） |
| **pid 0 拒发** | `send_signal(0, ·)` / `signal_group(0, ·)` 一律拒绝（`kill(0,·)` = 给调用方整个进程组发信号） |
| 收集阶段不杀 | `merge_descendants` 纯计算，不产生任何 kill（杀全部推迟到最后一步） |
| 输出截断 | 超限文件截断后 size ≤ cap，且**尾部内容保留**、头部丢弃 |
| 截断后子进程续写不产生空洞 | `O_APPEND` 下 truncate → 写 → 文件 size 连续（无 NUL 空洞） |
| **截断重写 fd 也是 append** | 宿主写回尾部后，子进程续写落在尾部**之后**（不被宿主覆盖） |
| 停止幂等 | 对 `Exited` 服务 stop → `stopped:false`，不报错 |
| 就绪超时 | 进程不输出 readyPattern → 走停止流程，返回「就绪超时」 |
| **registry 基线断言** | `tools_table_count_matches_constant`（含 `expected` 集合 + `expected_order` 顺序）与 `mutating_tools_matches_baseline` 全绿——改了 `TOOLS_TABLE` 没同步基线会红（§7.6） |
| **feature 守卫** | `TOOLS_TABLE` 含 cfg-gated 条目时当场红（§7.6 的易碎前提） |

### 16.2 集成测试（真进程，`src-tauri/tests/` 或 lib 内 `#[cfg(test)]`）

| 用例 | 断言 |
|---|---|
| **P0 全链路（第一条就要有）** | 走**真实 Skill DSL 链路**（`tests/skill_e2e.rs`）：SKILL.md 的 step1 调 `start_service` → step2 用 `${step1.serviceId}` 调 `stop_service` → 服务真的被停掉（登记表转 `exited`）。**不许只在单测里验变量替换**——单测验的是 `substitute_vars`，验不到"调度器 → 变量替换 → 工具入参解析 → 授权闸 → 真进程 → 登记表"这条链上任何一环的问题 |
| 起服务并存活 | `start_service` → `Running`；`list_services` 能看到；进程在 2s 后仍活 |
| **限额不继承**（核心回归） | 服务跑一个烧 CPU 的 node，**跑 5s 仍存活**；对照：同样脚本在 `run_python` 里 2s 就被 SIGXCPU 杀 |
| 停止阶梯 | `stop_service` → 进程消失；`method` 与实际信号一致 |
| **子孙回收** | 服务起一个再 spawn 一个 `setsid` 的孙进程；停父 → **孙进程也消失** |
| **PID 复用不误杀** | 停服务期间制造 pid 复用（起大量短命进程耗尽 pid 空间）→ **无关进程不被杀**，且记 `service.pid_reuse_skipped` |
| 僵尸 | 服务自己退出 → 登记表 `Exited` + `exit_code` 有值（watcher 生效，进程表里无 `<defunct>`） |
| 退出清理 | mock app + 起服务 → `cleanup_on_exit` → 进程死 + `app_exit_cleanup` 审计含 `services_stopped` |
| 权限闸 | 开关关 → `Denied`；`perm_mode=Yolo` → 放行；命令 `bash` → 拒绝 |
| cwd 闸 | cwd 在白名单外 → 拒绝（后台模式）或弹窗（交互模式） |
| 输出上限 | 服务狂写 stdout → 文件 size ≤ cap；`list_services` 仍能拿到最近 20 行 |
| **就绪快照不被截断吃掉** | 服务启动打横幅 → 之后狂写 stdout 把横幅挤出 8MB → `list_services` 仍能拿到 `ready_snapshot` 里的监听地址 |

### 16.3 全链路测试的覆盖边界（实施后确定）

上面那条「P0 全链路」用例实施时撞上一个硬约束：**本仓测试只有 mock runtime**
（`tauri::test::mock_app()`；Wry 需要显示环境 + 主线程，见 `tests/skill_e2e.rs` 头部说明），
而 **dispatch 层与 `bot_fs` 路径闸整体是 Wry 专属**（`ToolCtx.app` 收 `&AppHandle`，
`bot-config.json` 读写与授权弹窗都绑 Wry）。

处置：**把两处 Wry 依赖做成注入项**，让工具本体不绑 Runtime——
权限档（`yolo`）与路径闸（`resolve_cwd` 闭包）由调用方算好传进来
（与 `run_skill_scheduler_core` 注入 `execute_tool` / `persist` 同款思路）：

```rust
// bot/tools.rs
pub(crate) async fn tool_start_service<R, F>(
    app: &AppHandle<R>, args: &str, session_id: Option<&str>,
    yolo: bool, resolve_cwd: F,
) -> ToolResult
where F: Fn(String) -> Pin<Box<dyn Future<Output = Result<PathBuf, String>> + Send>> + Send + Sync
```

于是**真工具**能在 mock runtime 下被真调用，测试覆盖到：

| 环节 | 覆盖 |
|---|---|
| 调度器步骤推进 / 变量替换（`${step1.serviceId}`） | ✅ |
| 工具入参解析（JSON → `StartRequest`） | ✅ |
| 授权闸（设置页开关 + 启动器白名单） | ✅ |
| 真进程起停 / 登记表状态机 / 停止阶梯 | ✅ |
| **dispatch 层三道闸**（pre_tool 钩子 / per-tool 规则表 / 子 agent 白名单） | ❌ Wry 专属，dispatch 自身测试覆盖 |
| **`bot_fs` 路径授权闸**（白名单外弹窗） | ❌ Wry 专属，测试里用固定解析器替身 |

> 诚实标注：这条用例**不是**「全链路无盲区」。它证明的是
> 「skill 写的一步能真的起一个受管服务、下一步能真的用上一步的 serviceId 把它停掉」，
> 这正是本功能的核心承诺。dispatch 三闸与路径闸是**全仓所有工具共用的基建**，
> 它们的测试在各自模块里，不在本功能的账上。

### 16.4 门禁
```
bash scripts/test-fast.sh     # pre-commit
bash scripts/test-all.sh      # pre-push（cargo nextest + vitest）
cd src-tauri && cargo clippy --manifest-path src-tauri/Cargo.toml
```

### 16.5 无新增依赖

- 进程 spawn：`std::process::Command`（已有）。
- 进程组/信号：Unix 用 `libc`（**已在依赖里**，`py/runtime.rs` 用 `libc::setrlimit` / `kill`）。
  Windows 用 `taskkill`（**已在用**，`py/runtime.rs:301-305`）。
- ps 快照：调 `ps`（`py/runtime.rs` 已在用 `kill` / `taskkill` 这类系统命令，无新依赖）。
  取 `pid` / `ppid` / `lstart`（macOS）或读 `/proc/<pid>/stat`（Linux）拿 `starttime`。
- 文件操作：`std::fs`（已有）。
- JSON：`serde_json`（已有）。

---

## 17. 分期

| 期 | 内容 | 价值 |
|---|---|---|
| **P0** | 登记表 + `start_service` / `stop_service` / `list_services` + 停止阶梯 + 子孙回收 + 退出清理 + 权限闸（开关 / 白名单 / cwd）+ **限额不继承** + **输出上限巡检**（8MB 截断 + 30s 一轮） | 打通"skill 内部起 node 长驻"的主链路 |
| **P1** | **readyPattern + 就绪超时**（已提前落地：就绪判据是长驻服务的核心可用性，一并做了）+ **RSS/CPU 观测告警**（已落地：整棵进程树合计口径，单服务 512MB / 合计 1GB / CPU 跑满持续 5min，只告警不杀）+ **前端服务面板**（已落地：设置页「本地服务」分区，看状态与资源占用、随时停） | 可用性与可观测性 |
| **P1 剩余** | 资源告警已走**系统通知**（`bot/services.rs` 的 `notify_system`，插件没装时静默跳过）；**应用内通知中心仍未接**服务资源告警（`src/lib/notifications.ts` 的 kind 只有 memory_proposal / artifact_bind / task_question） | |
| **P2** | ① 方案 1：node MCP server 配到设置页——**已重定义并完成可见性**（见 §17.1 决策记录）<br>② MCP stdio 接同一停止阶梯（上提为 `bot::procs::stop_tree`）——**已落地**<br>③ 端口/监听地址硬化闸——**判据已定**（见 §17.2；实现排在可见性之后，共用同一份 socket 解析） | 配套与深化 |

### 17.1 P2-① 决策记录：node MCP server 归到「可见性」口径

**结论**：P2-① 的范围**重定义**为「宿主对 MCP 子进程的资源可见性」，
本节落地后按「**已重定义并完成可见性**」结案（不写"已完成"——原措辞太模糊，
"已有整套 UI"这种话会被反复拿出来重问）。

**为什么重定义**：原条目「node MCP server 配到设置页」混了三层意思，
不拆开会一直谈不清：

| 层 | 含义 | 结论 |
|---|---|---|
| (a) 能不能配 | 用户在设置页配一个 node MCP server | **已覆盖**（引用见下） |
| (b) 管不管得住 | 宿主知不知道、能不能停、退出时收不收干净 | **已覆盖**（P2-②，引用见下） |
| (c) 看不看得见 | 资源占用 / 运行时长 / pid | **本节的范围** |

**(a) 能不能配 —— 已覆盖，引用写死**（不再写"已有整套 UI"这种模糊话）：

- 配置面：`src/components/SettingsPage/McpPanel.tsx` —— stdio 表单的 command 来自
  `src/components/SettingsPage/types.ts` 的 `MCP_STDIO_LAUNCHERS` 下拉，
  有 args / env（值走 keyring）/ 超时；
  **保存前弹窗展示完整命令行**让用户确认。
- 配置结构：`src-tauri/src/bot/mcp/config.rs` 的 `McpServerConfig`
  （`transport` = stdio | http；`env` / `headers` 的值 `skip_serializing`，落 keyring）。
- 白名单：后端硬闸 `bot/mcp/config.rs` 的 `STDIO_LAUNCH_ALLOWLIST`
  （保存校验、错误文案、进程构造全查此表）。
  后端两份清单（白名单 ↔ `bot/mcp/manager.rs` 的逐臂字面量构造）由单测
  `stdio_allowlist_and_spawn_match_stay_in_sync` 锁住（新增白名单项漏改构造臂
  会静默断 stdio 路）。
  **但前端 `MCP_STDIO_LAUNCHERS` 是手工维护的第二份副本**——两侧当前一致
  （各 10 项），**没有测试锁**（该单测只管 Rust 侧）。见 §18 风险 14。

**(b) 管不管得住 —— 已覆盖，引用写死**：

- 生命周期：连接槽 + 按需连接 + 懒重连（`bot/mcp/manager.rs` 的 `ensure_connected`）。
- 停止：P2-② 起，MCP stdio 子进程**自建进程组**（否则整组杀会连宿主一起带走），
  断开/退出时先走 `bot::procs::stop_tree` 的同一套信号阶梯
  （SIGINT→100ms→SIGTERM→400ms→SIGKILL）+ 回收脱离进程组的子孙，
  **再**取消令牌让 rmcp 收尾（此时已是死进程，Drop 的 fire-and-forget SIGKILL 是空操作）。
  顺序不能反：rmcp 先杀会让逃逸子孙在快照前被 reparent，ppid 树就认不出它是谁的后代。
- 退出必杀：`src-tauri/src/lib.rs` 的 `cleanup_on_exit_with` →
  `bot::mcp::manager::shutdown_all`；审计 `app_exit_cleanup` 的 `mcp_stopped` 字段。
- 测试：`bot/mcp/manager.rs` 的 e2e
  `shutdown_stops_stdio_child_and_its_escaped_descendant`
  （桩服务器拉一个 setsid 逃逸子孙，断言 shutdown 后父子两个 pid 都不在了）。

**采样口径（本节要做的事）**：

- 与受管服务层**同一套采样代码**：`bot/procs.rs` 的 `take_snapshot` + `tree_sample`
  —— 整棵进程树合计（组长 + 全部后代），因为 dev server 的活多半在子进程里，
  只看组长会漏掉真正在烧的那个。
- 触发方式：**按需采样 + 5s TTL 缓存**（在 `mcp_status` 里取）。
  不做 30s 后台巡检：MCP 是按需连接、连接数少，巡检大概率空转，
  且巡检要先问管理器「现在哪些连着」再把采样状态塞回 MCP 侧，引入后台线程与
  管理器锁的耦合。
- 5s TTL 的理由：`mcp_status` 可能被前端轮询，每次都拍一张全机 `ps`（~50ms）就重了。
- CPU 占用率与受管服务层**同一条公式**（`bot/procs` 的 `cpu_usage_pct`）：
  delta 累计 CPU 时间 ÷ delta 墙上时间。第一次采样没有前值 → 面板显示累计 CPU 时间，
  第二次（≥5s 后）起才有百分比。
- **监听信息（端口 / 地址）也在这一份采样里**：`listening` 字段（local_address:port）
  与资源读数同批采出，**P2-③ 与 P2-①(c) 共用同一份 socket 解析，一次采样两用**——
  两个条目各写一套 `/proc/net/tcp` 解析的话，就是重复采样 + 两套口径。
  归属：显示归 P2-①(c)（可见性），**拦截归 P2-③（闸）**，判据见 §17.2。
  **已落地**（`bot/procs.rs` 的 `listening_sockets` + MCP 面板的资源行）；
  闸那一侧还没挂（P2-③ 未实现）。

**非目标（明确不做）**：

- **不含对话内临时挂载 MCP server**（模型在对话里临时起一个 MCP server 挂上）。
  若要做，**另开条目**——那是另一个产品面，设计稿没有它就不塞进来。
- **不合并进受管服务表**（不做"一张表看全部长驻进程"）：MCP 进程归连接槽、
  生命周期是按需的。合并要么给服务层开"管道模式"（与「不占管道」正面冲突：
  孙进程占住管道会触发 drain 超时那条连坐杀组路径），要么自造 FIFO+文件尾的
  rmcp 传输（JSON-RPC 分帧从文件尾读是竞态的）。为了"一张表"去动 rmcp 传输层，
  是用架构纯洁性换真金白银的 bug 面。
- **不给 MCP server 加 CPU/内存硬限额**：同受管服务层口径——限额会误杀正在干活的
  进程，且 macOS 没有等价的 per-process 内存硬限额。只观测。
- **面板里不做实时曲线**（任务管理器式）：现在没有需求，真要做再上巡检。
- **预设模板（选项 4）暂缓、单独开条目**：预设带第三方包名与版本固定，
  是"配置即信任"的一次性确认点，值得单独审文案与版本策略，不混进可见性里稀释 review。
  条目内容见 §17.3。

### 17.2 P2-③ 判据：端口/监听地址硬化闸（只管闸，可见性已归 P2-①(c)）

**范围纠正**：本条原本把两件性质不同的事混在一起——
「登记监听端口供面板显示」是**可见性**（与 P2-①(c) 同一层，已并进 §17.1 采样口径），
「限制绑定哪些地址/端口」才是**闸**（本节）。混在一起的后果是两条目各写一套
socket 解析——重复采样、两套口径。所以本节**只定闸**。

#### 三条判据

| 级 | 判据 | 定性 |
|---|---|---|
| **L1** | 默认只许 loopback：监听地址必须是 `127.0.0.0/8` 或 `::1`。绑 `0.0.0.0` / `::` / 任何非 loopback 地址即违规 | **真安全边界**。暴露到局域网/公网才是要防的 |
| **L2** | 保留端口：禁止绑宿主登记的端口 | **防抢占 / 防伪装**。模型起的服务不能占宿主端口 |
| **L3** | 特权端口 `<1024`：发现即终止 | **诊断而非安全边界**。非 root 子进程本来绑不上，价值在报错清楚 |

**L1 的放开方式**：独立开关「允许服务暴露到非 loopback」，**默认关**。
落地形态与现有三道闸并列，成为**第四道闸**（现有三道都在 `bot/tools.rs` 的
`tool_start_service`）：
闸 1 独立开关 `services::enabled`（flag 文件 `services-enabled.flag`）、
闸 2 启动器白名单 `STDIO_LAUNCH_ALLOWLIST`、
闸 3 工作目录过文件授权闸（`bot_fs::resolve_with_perm`，注入点
`bot/registry.rs` 的 `call_start_service`）。
**L2 / L3 不设开关**，直接拒。

**L2 保留端口表的来源**（维护一张表，每次采样前刷新）：

- 宿主本地 API 端口：`crate::api::API_PORT` = 4763（`src-tauri/src/api.rs:23`）。
- 运行时宿主自身正在监听的端口（动态采集——插件、可选服务器都算）。
- **「MCP 控制端口」在本仓库并不存在**：MCP HTTP 传输只允许公网
  （`bot/mcp/config.rs` 的 `http_url_is_public` 拒内网与 localhost 家族），
  本地没有「MCP 控制端口」这一说。stdio MCP server 内部自己开的端口属用户
  信任范围（见下方适用范围口径）。

#### 执行机制：检测-终止，不是预防

- **spawn 前**：解析命令行常见 flag（`--port` / `--host` / `--listen` / `--bind`）
  做快速失败。**仅为诊断优化，不是执行点**——模型可以写配置文件、设环境变量、
  在代码里硬编码端口，静态解析不可靠，当成闸会漏。
- **spawn 后**：前 10s 加密采样（0 / 0.5 / 1 / 2 / 5 / 10s），之后并入 30s 巡检。
  30s 巡检已有且**只有一条**：`bot/services.rs` 的 `start_output_sweeper` →
  `sweep_round`（同轮已跑 `enforce_output_caps` + `sample_resources`），P2-③ 的
  监听检测并进这一轮，不另起循环。
- **怎么取监听 socket**：读 `/proc/<pid>/fd` 的 socket inode → 匹配
  `/proc/net/tcp{,6}` 与 `/proc/net/udp{,6}` 的 inode 列 → 取 `local_address:port`。
- **平台现实**：上述 `/proc` 路径**只在 Linux 存在**。本应用发 macOS 与 Windows
  两版，`bot/procs.rs` 已经是「Linux 走 `/proc`、其余走 `ps`」的双平台结构，
  socket 这块同样要分平台。**判据（哪些地址/端口算违规）跨平台一致**，
  只有「怎么取到 socket」按平台分：
  - **Linux**：`/proc/<pid>/fd` 的 inode 配 `/proc/net/tcp{,6}`、`udp{,6}`
    （内核按 `__be32` 打印地址，**每 4 字节组内是反的**——`::1` 打印成
    `...01000000`，`fe80::` 打印成 `000080FE...`）。
  - **macOS**：`lsof -nP -a -p <pids> -iTCP -sTCP:LISTEN`（一次调用带全部 pid）。
    `*:7000` 这种通配写法靠 `TYPE` 列区分 IPv4 / IPv6 → 落成 `0.0.0.0` 或 `::`
    （不落通配符：判 loopback 靠的是具体地址）。
  - **Windows**：**没有路径**。`lsof` 不存在，而现有进程快照层在 Windows 上
    本来就退化了（它依赖 `ps`）。要做得走 `netstat -ano`，**另开条目**。

  这份解析已在 P2-①(c) 落地（`bot/procs.rs` 的 `listening_sockets`），
  **闸与可见性共用这一份**——两边各写一套 socket 解析就是重复采样 + 两套口径。
- **违规处理**：走既有信号阶梯杀进程组（`bot/procs.rs` 的 `stop_tree`），
  写审计事件，面板标红并显示违规地址/端口。
- **最坏检测延迟上界**：启动期 10s、稳态 30s。**这不是容忍暴露期**——
  一旦某次采样发现违规**立即终止**，不等窗口结束。加密采样点
  （0 / 0.5 / 1 / 2 / 5 / 10s）是为了**覆盖启动慢的服务**（进程起来到
  bind 落定之间可能隔好几秒），不是给违规留缓冲。
  真要「预防」只能上 network namespace——代价是服务对外不可达，**不做**。
- **UDS 不管**：unix domain socket 的文件落在 cwd，已有路径闸覆盖。
  P2-③ 只管 TCP / UDP。

#### 违规后不内置自我修正（发现即终止）

L1 违规**不给模型自我修正的机会**：发现即终止，直接走信号阶梯杀组，
诊断信息一次到位（违规的 `local_address:port`、L1 规则、改绑示例
`--host 127.0.0.1`）。模型下一轮自然改对。四条理由：

1. **与闸的前提矛盾**：闸的全部理由是「模型给的命令不可信」。让不可信方
   自证"我改好了"，宿主还得再采样确认一遍——等于把信任点又还回去。
2. **交互上不成立**：起服务是一次同步 tool call。要支持自我修正，tool call
   得挂起等模型下一轮，而模型下一轮要看到失败反馈才能改——那本质上还是
   「杀 + 返回错误 + 模型重试」，只是把"杀"推迟了，没省掉任何东西。
3. **重试路径本来就存在**：模型收到 tool 错误后重调 `tool_start_service`
   是 agent loop 的天然行为。「自我修正」是**错误信息质量**的职责，
   不是闸的职责。
4. **延迟不能翻倍**：检测-终止已有 10s / 30s 的最坏检测延迟，再叠一层等待，
   暴露时间直接翻倍。安全边界类闸，简单可预测比"人性化"重要。

#### 适用范围口径（防下次再混）

**P2-③ 只管受管服务**（命令由模型给）。**MCP stdio 子进程不做闸、只做可见性**——
MCP 的信任源是**用户手填 + 弹窗确认**，不是模型；且很多 MCP server 本身就要开端口
（开端口是它的正常功能，拦掉等于把这类 MCP server 全部作废）。
这句写在这里，否则下次还会被问「为什么 MCP 不拦」。

#### 非目标

- **不含 network namespace 级预防**（真预防的代价是服务不可达）。
- **不含 MCP 侧闸**（理由见适用范围口径）。

### 17.3 选项 4（backlog，不排期）：MCP server 一键预设模板

**状态**：开条目，**现在不做**，不排期。

- **范围**：`McpPanel` 加预设下拉，选中后填入 `command` + `args` + `env` 默认值；
  **仍走现有弹窗确认**，不绕过白名单闸。
- **信任点**：预设含第三方包名 → **必须 pin 版本**（`pkg@x.y.z`）；
  「版本升级需人工 review」写进条目本身。
- **待定**：预设集合（filesystem / fetch / memory / git 等）；
  是否允许用户自定义预设——**建议 v1 不允许**，否则自定义预设变成任意命令注入面；
  UI 文案。
- **明确不做**：不自动安装包、不自动 `npx` 拉最新版。
- **依赖**：无，可独立排期。

---

## 18. 风险与已知不足

| # | 风险 / 不足 | 现状判断 | 缓解 |
|---|---|---|---|
| 1 | 服务可自启动后 `setsid` 逃逸 | 增量快照（每级信号前拍一张）+ 递归回收能抓到**绝大多数**逃逸（§10.3）；但逃逸恰好发生在最后一次快照到 SIGKILL 之间的窄窗口仍可能漏 | 已尽力；这是 Claude Code 自己也还没堵完的洞（claudeissues #102511） |
| 2 | 内存无硬限额（macOS 无等价机制） | 只观测 + 告警 | §5.4 实测：正常 vite dev server 145MB/棵，4 个并发上限打满 ~580MB；告警阈值按实测定（单 512MB / 合计 1GB）；退出必杀是硬边界 |
| 3 | CPU 无硬限额 | 只观测 + 告警 | 杀正在构建的 dev server 比让它跑飞更糟 |
| 4 | 会话结束不回收服务 | §13.4 的取舍 | `stop_service` + 观测告警 + 用户可随时停 |
| 5 | 服务可监听任意端口 | P2-③ 判据已定（§17.2）：L1 默认只许 loopback、L2 禁占宿主端口、L3 禁特权端口。**实现未落地**，且它本质是**检测-终止不是预防**（最坏检测延迟上界 10s/30s，发现即终止、不等窗口结束）；真预防要 network namespace，代价是服务不可达 | 现状缓解：审计记全 argv + `list_services` 展示 stdout + **就绪快照固化监听地址**（§11.5）。落地后：违规走信号阶梯杀组 + 审计 + 面板标红；L1 可用独立开关放开 |
| 6 | 停止阶梯期间服务可能写坏自己的文件 | SIGINT 给 100ms 优雅退出，SIGKILL 是兜底 | 语义与 Claude Code 一致 |
| 7 | 并发上限 4 是拍的 | 无依据数据 | 可配（设置页），先给个保守值 |
| 8 | `ps` 快照在超多进程机器上慢 | 单次 ~50ms 量级 | 只在停止时拍（低频，3 次），不在巡检里拍 |
| 9 | 登记表无上限 | 每条几百字节，会话内起 1000 个也就几百 KB | 可接受；真要防就加"只留最近 N 条终态" |
| 10 | **PID 复用导致漏杀** | 同一 pid 在两次快照间被复用 → 剔除不杀（§10.3） | **刻意选择**：宁可漏杀也不误杀用户的进程。漏掉的那个进程已经不是我们的子孙；记 `service.pid_reuse_skipped` |
| 11 | **日志截断窗口内的输出可能丢失 / 顺序错乱** | `set_len(0)` 与写回之间有几毫秒窗口（§11.3） | 接受：日志尾部本就是"最近发生了什么"的近似；关键信息（启动横幅 / 监听地址）由 `ready_snapshot` 单独固化（§11.5），不依赖日志 |
| 12 | **DSL 变量替换：数字落成字符串** | `resolve_nested_path` 对非 String 终值走 `to_string()`，数字 `5173` → `"5173"`（§7.5） | 既有实现的性质，非本方案引入。**占位符写在非字符串位时实测能落回真正的 JSON 数字/布尔**（探针 2：`is_number()==true`）；写在字符串位则落字符串。已写进 §7.5 的示例 |
| 13 | **DSL 变量替换：残留占位符传给下游** | key 不存在时 `${step1.nope}` **原样保留**并随 args 传给下游工具，下游看到的是字面量而不是"缺参数" | 既有实现的行为（"不静默变空串"是有意的）。下游多半会报"URL 非法"之类的错，**可诊断性中等**。SKILL.md 侧靠"先跑 step1 看返回"来规避；根治要改 `vars.rs` 的 fallback 策略（超出本方案范围） |
| 14 | **前端启动器清单与后端白名单是两份手工副本** | 后端硬闸 `bot/mcp/config.rs` 的 `STDIO_LAUNCH_ALLOWLIST` 与前端下拉 `src/components/SettingsPage/types.ts` 的 `MCP_STDIO_LAUNCHERS` **当前一致**（各 10 项），但**没有测试锁这一对**（`stdio_allowlist_and_spawn_match_stay_in_sync` 只锁 Rust 侧白名单↔进程构造臂） | 前端多一项 → 用户能选但后端保存时被拒（报错清晰，不静默）；后端多一项 → 用户在 UI 里选不到（功能缺失，不报错）。两侧都靠人手记，迟早漂。修法：加一个跨语言源码锁单测（照 `src/lib/taskFiles.test.ts` 直接读 Rust 源文件的做法，把 `STDIO_LAUNCH_ALLOWLIST` 解析出来与 TS 常量对拍）——**另开条目**，P2-① 只做可见性不扩这个 |
| 15 | **端口闸只在受管服务侧，MCP stdio 子进程不受约束** | 刻意如此：MCP 的信任源是用户手填 + 弹窗确认，不是模型；且很多 MCP server 本身就要开端口（开端口是它的正常功能） | 口径写进设计稿 §17.2 适用范围，避免下次又被问「为什么 MCP 不拦」 |

---

## 19. 附录：实测记录

### A. RLIMIT_CPU 继承与 SIGXCPU

脚本（`/tmp/wm_limit_probe.py`，测完已删）：

```python
import resource, subprocess, time, signal
soft, hard = resource.getrlimit(resource.RLIMIT_CPU)
resource.setrlimit(resource.RLIMIT_CPU, (2, hard))
probe = "console.log('node ulimit -t =', require('child_process').execSync('ulimit -t').toString().trim())"
subprocess.run(['node','-e',probe])
burn = "const t=Date.now();while(Date.now()-t<10000){Math.sqrt(Math.random())}"
p = subprocess.run(['node','-e',burn])
print(f"rc={p.returncode} ({signal.Signals(-p.returncode).name})")
```

输出：

```
parent RLIMIT_CPU = (2, 9223372036854775807)
child says: node ulimit -t = 2
burn node rc=-24 (SIGXCPU) elapsed=2.0s
```

### B. node 虚拟内存足迹与 macOS 的 RLIMIT_AS

```
$ ps -o pid=,vsz=,rss=,comm= -p <node pid>
32120 436127312  42432 node
```

VSZ ≈ 416 GiB / RSS ≈ 42 MB。

```
$ ulimit -v 524288
/bin/bash: line 11: ulimit: virtual memory: cannot modify limit: Invalid argument
```

### C. 内存占用实测

```bash
# 最小 node http server
node -e 'require("http").createServer((q,s)=>s.end("ok")).listen(0); setInterval(()=>{},1000)' &
sleep 1.5 && ps -o pid=,rss=,vsz= -p $!
# → RSS=44.1 MB  VSZ=415.9 GB

# 仓库真实 vite dev server（vite 7.3.6）
node node_modules/vite/bin/vite.js --port 5199 --strictPort &
sleep 7 && ps -o pid=,rss=,vsz= -p $(pgrep -f "vite.js --port 5199")
# → RSS=127.0 MB  VSZ=424.1 GB
# 子进程（esbuild 服务）→ RSS=17.6 MB
# 整棵进程树合计 → 144.6 MB / 2 个进程

# 宿主侧 watcher 线程成本（200 个线程）
# → RSS 增量 ≈ 0（每个约 0.1 KB），VSZ 增量 0.4 MB

# ps 快照规模（本机）
ps -o pid= -o ppid= -ax | wc -l
# → 757 行
```

### D. 关键源码坐标

| 事实 | 坐标 |
|---|---|
| 进程组创建 | `py/runtime.rs:789` `cmd.process_group(0)` |
| rlimit 设置 | `py/runtime.rs:800-825`（`pre_exec`） |
| `mem_limit_bytes` / `cpu_limit_secs` | `py/runtime.rs:55-60` / `:62-64` |
| `setrlimit` 探测机制 | `py/runtime.rs:588-761` |
| 连坐杀组 | `py/runtime.rs:205-229` `kill_tree` |
| drain 超时连坐 | `py/runtime.rs:941-946` |
| 跑完删目录 | `py/runtime.rs:957` |
| 父进程看门狗 | `py/runtime.rs:322-330` |
| `PY_CHILDREN` / `kill_all_py_children` | `py/runtime.rs:265-279` / `:313-319` |
| `EXITING` / `mark_exiting` | `py/runtime.rs:425-429` |
| Python 开关（flag 文件） | `py/commands.rs:10-34` |
| yolo 免开关口径 | `py/document.rs:2003-2006` |
| 启动器白名单（单一事实源） | `bot/mcp/config.rs:49-51` |
| 白名单校验 + 逐臂构造 | `bot/mcp/manager.rs:261` / `:268-287` |
| MCP 进程 spawn / shutdown | `bot/mcp/manager.rs:254`（spawn_service）/ `:312`（child pid）/ `:714-724`（shutdown）/ `:755`（shutdown_all） |
| 进程采样公共件（P2-① 与受管服务层共用） | `bot/procs.rs:118`（take_snapshot）/ `:303`（tree_sample）/ `:287`（cpu_usage_pct） |
| 监听 socket 枚举（P2-① 落地，P2-③ 复用） | `bot/procs.rs:365`（listening_sockets）/ `:385`（tree_pids）/ `:392`（merge_sockets）/ `:471`（parse_proc_net）/ `:600`（parse_lsof_line） |
| MCP 子进程资源采样（P2-①） | `bot/mcp/manager.rs:420`（connected_children）/ `:442`（sample_children，5s TTL）/ `:447`（sample_children_within） |
| MCP 面板资源字段（P2-①） | `bot/mcp/commands.rs:216`（McpServerStatus）/ `:244`（ListeningBrief）/ `:263`（mcp_status，采样走 spawn_blocking） |
| `ToolDef` / `TOOLS_TABLE` / 工具数 | `bot/registry.rs:164-178` / `:1776+` / `:1772` |
| 工具名基线断言（集合 / 顺序 / mutating） | `bot/registry.rs:2795` / `:2868` / `:2935` |
| profile 工具列表 | `bot/registry.rs:2368` / `:2378` / `:2388` / `:2401` |
| dispatch 三道闸 + 分发 | `bot/dispatch.rs:395-407` / `:431-464` / `:559-566` |
| 路径授权闸 | `bot_fs.rs:317` `resolve_with_perm` |
| `AppState` / `skill_runs` 访问器 | `app_state.rs:82-113` / `:145-149` |
| 退出清理 | `lib.rs:222-248` `cleanup_on_exit_with` |
| Skill 变量替换 | `bot_skills/scheduler.rs:440,548` + `bot_skills/vars.rs:29-30,184-198` |
| 历史取舍记录 | `DEVLOG.md:4146`（drain 必须无条件组杀）/ `DEVLOG.md:4150`（setsid 逃逸归入"记录不修"） |
