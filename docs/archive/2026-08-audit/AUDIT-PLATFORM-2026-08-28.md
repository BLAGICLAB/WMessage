# Rust Bot 全面审计 · 批次 6：跨平台与资源（2026-08-28）

范围：`bot_py.rs`（Python/.NET 子进程生命周期、py-runs 临时目录）、`audit.rs`（审计写入、
probe_dir 便携目录探测）、`db.rs`（日志轮转）、`lib.rs`、`build.rs`、`tauri.conf.json`、
`bot_skills/`（skill_runs）。

方法：explore 子代理扫描（其 Read 工具故障，bot_py.rs 只覆盖前 1000 行）→ 主线程补读
未覆盖区（run_doc_revisions 回退链、rotate_log_if_large、skill_runs、kill_tree）+
逐条源码复核。

## 发现汇总

| # | 级别 | 位置 | 结论 | 处置 |
|---|------|------|------|------|
| G1 | P1 | audit.rs:278-290 | `probe_dir` 把可写的 macOS `.app/Contents/MacOS` 当便携数据目录：dmg 拖到 ~/Applications 后数据库/AI_Gen_Files/bot.log 全写进 app 包内（破坏签名、删 app 即删全部数据） | **已修**：`is_macos_app_bundle_dir` 跳过便携分支 |
| G2 | P1 | bot_py.rs:205-209 | `env!("CARGO_MANIFEST_DIR")` 把构建机绝对路径烧进发布二进制（信息泄露 + 发布版纯死路径） | **已修**：开发模式候选仅 `cfg(debug_assertions)` 保留 |
| G3 | P1 | bot_py.rs:196-237 + 绿色包 | 绿色包是 framework-dependent 构建却不含 .NET 运行时，「免装 .NET」未达成；运行侧只认 PATH 里的 `dotnet` CLI，不会直跑随包 apphost exe | **代码已修**：优先随包 `wm-docx-revisions(.exe)` 直跑（self-contained 即免装运行时；失败自动回退 Python 链路复核完整）；**打包侧待老板改** `dotnet publish -r win-x64 --self-contained` |
| G4 | P1 | bot_py.rs:819-838（Unix 分支） | macOS 无 Job Object/KILL_ON_JOB_CLOSE 等价物：主进程崩溃（非 ExitRequested）时 Python 子进程成孤儿，RLIMIT_CPU 限的是 CPU 时间——睡眠型失控脚本可永久驻留 | **已修**：Unix 脚本注入父进程看门狗前导（ppid 变 1 = 父死即自退，2s 轮询 daemon 线程） |
| G5 | P2 | bot_py.rs:438-456, 532-548 | `kill -9 -pid` 按裸 pid 杀进程组：pid 被 OS 回收复用后可能命中无关进程组 | **部分修**：组首校验只进 `kill_py_children`（退出清理，距收割久、复用窗口真实）；`kill_tree` 的 drain_timeout 路径必须保持无条件组杀（组首死后 pgid 随存活孙进程仍有效，不杀则孙进程占管道 reader 永不 EOF——回归测试 `run_python_at_grandchild_pipe_readers_joined` 实锤） |
| G6 | P2 | bot_py.rs:123-141, 172-178 | macOS GUI（Finder 双击）PATH 极简，brew 的 python3/dotnet 探测假性失败 | **已修**：macOS 补固定路径候选（/opt/homebrew/bin、/usr/local/bin、/usr/local/share/dotnet） |
| G7 | P2 | audit.rs:149 | 审计 append 失败只 eprintln——GUI（尤其 Windows CREATE_NO_WINDOW）stderr 无人可见，盘满/权限变化时审计静默归零 | 记录（需前端可见性改造，发版排期） |
| G8 | P2 | tauri.conf.json | 安装包（nsis/dmg/deb）无 bundle.resources 不含 dotnet 工具（Python 兜底行为正确但静默）；`hardenedRuntime:false` + ad-hoc 签名；targets 含未实际支持的 deb | 记录（打包/发版决策，未动） |
| G9 | P3 | bot_py.rs:443 | 孙进程 setsid/double-fork 换组逃逸杀不掉（脚本有完整用户权限，非沙箱，已知 trade-off） | 记录 |
| G10 | P3 | db.rs:126-133 | 日志轮转只留一代 .old（5MB+5MB 封顶），有界 | 无需修 |
| G11 | P3 | — | py-runs 正常/失败路径全部清目录（setup_fail 自清 + cleanup_after_fail + 成功删），启动 sweep 1h 兜底；skill_runs 为内存态无磁盘泄漏；AI_Gen_Files 无上限但属用户产物 | 无需修 |

## 已核对无问题

- 进程树清理设计对称且完整：Unix `process_group(0)`+进程组杀；Windows Job Object +
  KILL_ON_JOB_CLOSE（主进程崩溃 Job 句柄关闭即整树强杀，崩溃路径 Windows 有兜底）+
  taskkill /T /F 双兜底；ChildRegGuard RAII 覆盖全部返回路径。
- dotnet→Python 回退链完整（run_doc_revisions：不可用/非零退出/Err 均回退并记审计）。
- 无 shell 拼接（全程 Command+数组参数），路径含空格/中文安全。
- capabilities/default.json 无新口子（批次1 收敛有测试锁死）；CSP 严格；版本单一真相源有测试。

## 修复清单（对应 commit）

1. **G1** audit.rs：`probe_dir` 跳过 `*.app/Contents/MacOS` 形态 + 2 个单测。
2. **G2/G3** bot_py.rs：`dotnet_revisions_dll` → `dotnet_revisions_entry()`（随包 exe 优先
   直跑，dll 形态才要求系统 dotnet）；dev 候选 cfg(debug_assertions)；
   `run_python_at` 的 entry 参数改 `Option<&str>`（exe 直跑无 dll 参数）。
3. **G4** bot_py.rs：Unix 下 `run_python_ungated` 给脚本注入 `PARENT_WATCHDOG` 前导。
4. **G5** bot_py.rs：`is_live_group_leader()` 组首校验接入 kill_tree / kill_py_children。
5. **G6** bot_py.rs：macOS 固定路径探测候选。

## 老板需要做的（打包流程，代码已铺好）

- 绿色包 .NET 工具改 self-contained 发布：
  `dotnet publish src-tauri/dotnet/WmDocxRevisions -c Release -r win-x64 --self-contained -o <绿色包>/dotnet`
  （现在体积会大几十 MB，换来用户免装 .NET；发布后继续用现在的 Python 兜底，双保险）

## 回归测试

- audit.rs：.app 包内目录跳过便携分支（真实建目录探测）+ is_macos_app_bundle_dir 判定
- bot_py.rs：debug 门控源码锁 / exe 优先源码锁 / 看门狗注入源码锁 / 组首校验（死 pid 不误杀）
