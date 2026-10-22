# wmessage 分模块审计账本（2026-10-22）

范围：按功能模块全量代码审计，共 15 模块、6 批。方法：全程只读不改码；每批并行
只读子代理分模块审，**P1 及以上由主会话逐条读码复核（行号+触发条件）后才记为事实**，
P3 为子代理报告（均附代码行）。修复不在本轮：全部发现逐条拍板后另开批次。

分级沿用 2026-10-21 口径：P0 破坏安全/数据边界；P1 功能断裂或承诺与代码背离；
P2 局部缺陷（有实际破坏面但触达受限）；P3 打磨/加固。

---

## 批 0 · 基线校准

- 工作区干净 commit（45c84aa1）
- `cargo nextest`：**1981 过 / 10 skip**；`vitest`：**985 过 / 92 文件**
- tests-audit 对拍：**29 过**；前端禁动态求值守卫 140 文件零命中
- `cargo clippy --all-targets -D warnings`：干净；`cargo machete`：无未用依赖
- 架构文档 §6.4 与真实文件树对拍：**157 = 157**，无幽灵条目、无漏登记
  （2026-10-21 报告的 3 幽灵 + 26 漏登记修复确认未回退）

发现：

- **[P3] `evolution/` 目录只剩 .DS_Store**（`src-tauri/src/evolution/`、
  `evolution/sandbox/`，无任何 .rs）——死目录，架构文档未登记是正确行为；
  建议随下一笔修复合伙清掉。

---

## 批 1 · 代码执行面（M4 程序 apps + Python 运行时 / M5 桌面 UI 自动化 / M3 补盲）

### M5 桌面 UI 自动化（bot_ui.rs + ui/ 三件）

四道闸结构上真实存在且接线完整（总开关默认关、动作时重验窗口、确认 60s 默认拒绝、
子 agent 白名单不含六件套、per-tool 规则闸生效），但**闸门判定材料全部可绕**。

**[P1-1] 危险动作确认关键词大小写敏感，英文按钮全部绕过**（主会话已复核）
- 位置：bot_ui.rs:115（关键词表 :40-67；detail 拼 title 于 :391-400 / :476-486）
- 触发：目标按钮标题是任何大写开头的英文（"Send"/"OK"/"Delete"——英文 UI 标准写法）。
  关键词全是小写字面量，`detail.contains(k)` 未 lower case → 不确认直接执行。
- 证据：`if !DANGEROUS_KEYWORDS.iter().any(|k| detail.contains(k))`；单测只测中文。
- 修法：detail lower case 后匹配；"ok" 等短词改词边界/全文匹配（防 "Outlook" 窗口误触发确认疲劳）。

**[P1-2] ui_type 不中和换行符，text 带 `\n` 即触发发送/提交且不走确认闸**（主会话已复核）
- 位置：bot_ui.rs:444-530（全程无 `\r\n` 过滤）；ui/macos.rs:365-392、ui/windows.rs:373-403（UTF-16 码元原样注入）
- 触发：`ui_type {text:"早上好\n"}` 到聊天输入框——回车提交类应用（微信/飞书/Electron 系）
  收到换行键即发送。危险闸只查 text 前 60 字符的关键词，不含关键词的「文本+回车」直接绕过。
- 证据：registry.rs:374 schema 明确承诺「输入完不会自动发出去——发送是另一次 ui_click」，代码无任何机制兑现。
- 修法：ui_type 入口剥除/拒绝 `\n` `\r`（或转成需独立确认的显式「按回车」动作）。

**[P1-3] 敏感控件判定不含 AXSecureTextField 角色与 UIA IsPassword，密码框可被代填**（主会话已复核）
- 位置：bot_ui.rs:70-79（关键词表）、:127-142（gate_sensitive）；ui/windows.rs（collect 不读 CurrentIsPassword，grep 全文件零命中）
- 触发：macOS 密码框 role="AXSecureTextField"（title 为空或不含关键词）→ hay 不命中 →
  放行 → focus + CGEvent 把密码打进密码框（键盘注入对安全框与真实键盘等效）。
- 证据：SENSITIVE_KEYWORDS 全是内容词，无角色判定；单测密码节点 title="密码" 恰好靠标题命中，掩盖盲区。
- 修法：gate_sensitive 增加角色判定（AXSecureTextField / UIA IsPassword==true 直接拒）。

**[P1-4] Windows 坐标点击兜底把像素坐标当 0-65535 归一化坐标用，落点失控**（主会话已复核）
- 位置：ui/windows.rs:316-322（press_node 兜底）、:332-361（click_at）
- 触发：ui_click 目标无 Invoke 模式（输入框等）走坐标兜底。`MOUSEEVENTF_ABSOLUTE` 要求
  dx/dy 为 0-65535 归一化值，代码直接传像素值 → 实际落点 ≈ (x/65535)×屏宽；
  x=500 落在距左边缘约 15px 处——一次不受控的真实点击（可能点中左上角任意控件）。
- 证据：`click_at((rect.left+rect.right)/2.0, ...)` → `dx: sx`（像素 i32）；
  函数自己的注释都写着「0-65535 归一化的绝对坐标」。多显示器无 VIRTUALDESK，只映射主屏。
- 修法：按主屏尺寸把像素换算为归一化值再注入（或 SetCursorPos + 相对点击）。

**[P1-5] ui_type/ui_click 不要求目标窗口在前台，键盘注入落进前台任意应用**（主会话已复核）
- 位置：bot_ui.rs:490-496（focus+type 两步，无置前）；ui/macos.rs:353-362（focus=AXPress，不激活应用）、:365-392（CGEvent post 到 HIDEventTap，路由给前台应用）
- 触发：目标窗口在后台（用户正在用浏览器/IDE）→ AXPress 可对后台窗口「聚焦成功」，
  但键盘事件全部进入前台应用——文本打进用户当前正在用的任何应用（另一聊天窗、终端、地址栏）。
- 证据：`focus_node(&node_id)?; type_text(&typed)` 两步之间无前台校验、无自动 activate。
- 修法：注入前校验目标窗口 focused（或先自动 activate_window），不一致即拒绝/先置前。

P2/P3：

- **[P2] macOS 窗口 id 是 `pid:AXWindows 下标`，可复用，窗口级 TOCTOU**（macos.rs:191-200/236-252；
  bot_ui.rs:92-105 gate_window 只查存在不比对标题）——窗口 A 关闭、同应用窗口 B 补位同下标 →
  旧 id 操作到窗口 B。修法：gate 命中后比对 title 或直接用句柄标识。
- **[P2] 闸门判定用快照节点，实际动作按 id 路径二次解析，界面重排后点/输到别的控件**
  （bot_ui.rs:145-188 vs :405-410 press_node 内部重新走树）——危险闸/敏感闸审的是旧控件，
  动的是新控件。修法：解析与动作合并一次，或动作后回读校验。
- **[P2] 无标题按钮完全绕过危险闸**（bot_ui.rs:391-400 fallback 到 role，永不命中关键词）——
  图标按钮（纸飞机发送/垃圾桶删除）不受确认；建议 fail-closed：无标题默认 ask。
- **[P2] 注入无全局互斥**，并发会话可交错 focus/type（bot_ui.rs:405/493 各自 spawn_blocking，
  全仓无注入锁）。修法：注入段套一把全局 Mutex。
- **[P2] 总开关状态变更不留审计**（ui/mod.rs:88-109 只有 fs::write/remove_file）；
  flag 文件 0644，本地任意进程可创建文件解锁该能力。
- **[P2→设计声明] 授权模型 = 机器上任何真实窗口**，「窗口级授权」实为「窗口存在性校验」，
  无应用白名单。至少提供可选应用白名单（默认仅显式添加的应用可动作）。
- **[P3]** 动作前截图失败完全静默（bot_ui.rs:190-207，注释声称记日志但无任何记录）；
  截图文件名毫秒时间戳可碰撞（对比 bot_desktop 的纳秒方案）；macOS list_windows `focused` 恒
  false、无标题窗口照列但工具文案称「只列有标题的」；"pin" 子串误伤（"Shipping"/"spinning"）；
  ui_click 全程无敏感控件判定（含 Windows 坐标兜底路径）；Windows HWND 复用低概率 TOCTOU；
  NEEDS-VERIFICATION：AX 权限中途吊销时 type_text 静默假成功（gate 过后、注入前的时间窗）。

已查证安全：ui_set_enabled 未暴露给 bot；确认 pending 以 UUID 键不互顶；子 agent 白名单不含六件套；
mutating 标记正确；失败/被拒动作留痕；wait_for_element 有界（30s 硬顶）；macOS 树遍历双重上限；
Windows COM 成对释放；底层注入函数无绕闸调用方；Windows inspect 不回传输入框值。

### M4 程序 apps + Python 运行时（apps/ 三件 + db/apps.rs + bot_py.rs + py/ 九件）

**[P2-1] `validate_app_id` 放行 `"."`，app 目录折叠到 apps 根，删除时可抹掉全部程序目录**（主会话已复核）
- 位置：apps/manage.rs:129-138（校验）、db/paths.rs:26-28（join）、apps/manage.rs:104-114（delete_core 的 remove_dir_all）
- 触发：`apps_create` 传 `id:"."`（校验只拒空/`/`/`\`/`..`）→ `apps_root.join(".")` 归一化为
  apps 根 → 脚本/DB 行都落在根上；归档后「彻底删除」→ `remove_dir_all(apps_root/.")` =
  **删除所有程序的目录**，其余 app 的库行全部悬空。schema 承诺 id 仅字母/数字/-/_。
- 修法：按 schema 收窄为 `[A-Za-z0-9_-]+` 白名单（或要求 components 恰一个 Normal）。

P3：

- **[P3] app_runs 僵尸行**：建产物目录失败/读脚本失败/DB 写失败/runner panic 等路径不回写，
  status 永停 running，全仓无 stale 清扫（apps/mod.rs:101-109/139/184）。
- **[P3] 「整文件集提交」实为合并不删除**，且附加文件不进版本快照、回滚不恢复附加文件
  （manage.rs:361-377/455-480/485-532）——注释声称「内容整体回旧版」与代码不符。
- **[P3] Python 子进程继承宿主全量环境变量**（py/runtime.rs:866-871 无 env_clear）——
  宿主环境里的密钥类变量对 AI 生成脚本可见。修法：env_clear + 显式白名单。
- **[P3] TMPDIR 未指向本次运行目录**：tempfile 落系统 /tmp 且退出不清理（多用户机器上
  params.json 可被同机其他用户读）。
- **[P3] 无进程数限额**：Unix 无 RLIMIT_NPROC、Windows Job 无 ACTIVE_PROCESS——fork 炸弹
  在运行窗口内可打满系统资源（超时杀组可兜底回收）。
- **[P3] apps_run 不接 StopToken**：UI 无法中途停止一次运行（最长 1800s；停止基础设施现成，纯接线）。
- **[P3] fs::write/读脚本跟随预置符号链接**（manage.rs:367、mod.rs:93-109）——需本地写权限预置，
  LLM 不可独立触达，纵深防御缺口；harvest.rs 有同款先例可抄。
- **[P3] `apps_open_output` 未重验 id 是路径分量**（commands.rs:145-156，隐式依赖 DB 存在性）；
  **[P3] `apps_run` 无 bot.log 审计事件**（只有 app_runs 表行 + 系统通知）。

已查证安全：script_rel_path 对 `..`/绝对/子目录/URL 编码/盘符/UNC 全拒且校验执行同值
（无 TOCTOU）；脚本读文本拷贝到 uuid 临时目录执行；RESULT 哨兵仅 stdout/取最后合法行/
300 字符截断不劈中文；manifest 全字段校验 + 附加文件锁死 `data/` + 字符集白名单；
版本滚动 100 边界正确、回滚语义完整；kill_tree 进程组/Job 全树杀 + pid 回绕护栏；
rlimit/Job 内存+CPU 限额 + S20 探测降级诚实留痕；harvest 软链不跟随；ast 沙箱路径内联转义
+授权闸先于 code 生成；db/apps.rs SQL 全参数化；bot 无 apps_run 工具（AI 不能自行执行 app）。

NEEDS-VERIFICATION（需拍板定性）：
1. apps 模块整体不受 `py-enabled.flag` 约束是否有意（bot 可无开关落 app 代码，执行需用户点击）。
2. `apps_run` 无审计事件是否满足审计口径（对比 py_exec 每次留痕）。
3. 若历史上已有畸形 id 落盘（`.`/空格/控制字符），修 validate 后需一次性清洗迁移——查 bot.log 取值分布。

### M3 补盲（bot_desktop + overflow + platform；bot_fs/bot_archive/bot_manage_file 上轮已审）

无 P0-P2。P3 四条：

- **[P3] `desktop.copy_file_to_clipboard` 成功审计行未转义**（bot_desktop.rs:266-269）——
  文件名可含换行伪造 bot.log 审计行；同文件 reveal/open_url 都走了 escape_for_log，唯独这处漏了。
- **[P3] open_url 对方括号 IPv6 字面量（`http://[::1]/`）误判为「DNS 解析失败」**（bot_web.rs:967-989）——
  url crate 的 host_str() 返回含 `[]` 的串，`parse::<IpAddr>()` 失败 → 落进 DNS → 报网络故障
  （fail-closed 但分类错误，公网 IPv6 字面量也打不开）。修法：剥 `[]` 再判。
- **[P3] 公网闸未覆盖 0.0.0.0/8 与广播段**（bot_web.rs:1307-1315）——字面量 `http://0.1.2.3/`
  可过闸交给系统浏览器（内核按本机处理；对 open_url 影响低，但同一道闸被 fetch_url 共用）。
  修法：ipv4_is_private 补 0/8、broadcast、documentation、reserved。
- **[P3] overflow 清扫任务无守护**（overflow.rs:75-90）——loop 内 panic 即永久停摆无重启；
  sweep/spill 均为内联同步 IO 跑在 async runtime 上（与 bot_fs 全线 spawn_blocking 纪律不一致）。
- **[P3] 清扫按 mtime 判龄的时钟边界**：系统时间前跳 >30 天会删掉刚转存未读的溢出文件
  （低风险，trace 侧同构）。
- **[P3] macOS 无屏幕录制权限时截图「成功」返回壁纸**——已文档化的平台限制，无需修码。

NEEDS-VERIFICATION：
1. open_url 的 DNS TOCTOU 与后续重定向由浏览器接管，闸看不到（fetch 侧有钉地址，opener 通道
   无等价手段）——固有边界；建议工具成功文案补「后续跳转与解析由浏览器接管」。
2. opener 插件 capability：Rust 侧调用天然绕过 ACL（真实闸就是白名单+公网闸，已确认有效）；
   但 `opener:default` 的 allow-open-url 无 scope——前端 webview 可 invoke 打开任意 URL/reveal
   任意路径。若前端有渲染不可信内容的场景应收窄为 deny-open-url（与批 5 前端审计联动定性）。

已查证安全：reveal_path 白名单闸（canonicalize+starts_with 按分量比较，`overflow-evil/` 前缀碰撞
不可能）；copy_file_to_clipboard 读闸真实存在且逐路径过 resolve_with_perm；剪贴板 10 万字符上限；
copy_file 无临时文件、失败分支 GlobalFree/EmptyClipboard 配平；screenshot 文件名纯程序生成毫秒+
纳秒双防撞、PowerShell 参数数组无注入面；overflow 原子写 + 只读白名单（不在写侧）；回灌上限
`min(per-tool, global)` 无「两处缺一」、截断回退 UTF-8 边界；strict 拒绝文案不泄露白名单全量。

---

## 批 2 · 网络与 API 面（M10 本地 HTTP API / M12 模型元数据·配置·密钥 / M15 跨平台横切）

（待跑）

## 批 3 · 核心数据链（M1 任务卡 / M11 图谱标签 / M14 底座迁移）

（待跑）

## 批 4 · 已审模块复验（M2 / M6 / M7 / M8 / M9）

（待跑）

## 批 5 · 前端专审（M13）

（待跑）

## 汇总与拍板清单

（收尾填写）
