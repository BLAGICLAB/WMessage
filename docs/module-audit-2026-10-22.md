# wmessage 分模块审计账本（2026-10-22）

范围：按功能模块全量代码审计，共 15 模块、6 批。方法：全程只读不改码；每批并行
只读子代理分模块审，**P1 及以上由主会话逐条读码复核（行号+触发条件）后才记为事实**，
P3 为子代理报告（均附代码行）。修复不在本轮：全部发现逐条拍板后另开批次。

分级沿用 2026-10-21 口径：P0 破坏安全/数据边界；P1 功能断裂或承诺与代码背离；
P2 局部缺陷（有实际破坏面但触达受限）；P3 打磨/加固。

**进度（2026-10-24）**：六批全部完成——批 0 基线 / 批 1 代码执行面 / 批 2 网络与 API 面 /
批 3 核心数据链 / **批 4 聊天决策与记忆技能面（M2+M6+M7，本轮补）** / 批 5 前端专审 /
**批 6 外部接入与服务进程面（M8+M9，本轮补）**。15 模块 × 审计状态全部落账，
总计 P0 0 / P1 9 / P2 27 / P3 95 / 待定性 15 处（详见文末「汇总与拍板清单」）。

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

### M10 本地 HTTP API（api* + api_handlers 九件）

无 P0-P2。安全设计成熟度高：回环硬编码、鉴权前置路由分发（仅 /api/health 免鉴权）、
token 122 位熵+0600+恒定时间比对+不进 URL、SSE 有界队列(256)+尸体三重清理、
body 1MB 上限+CL/TE 歧义全拒、RMW 基线 409、错误响应静态文案不回显内部。
P3 六条：

- **[P3] api-token.txt 空文件残留使 api_start 恒失败，「下次调用自愈」注释不成立**
  （api_auth.rs:42-62——读空走生成、create_new 撞 AlreadyExists、重试 50 次仍空。
  修法：读到空文件改原子覆写）。
- **[P3] 服务运行中 token 文件被删后 api_status 返回与活服务不一致的新 token**
  （commands.rs:300-309——磁盘新生成 UUID ≠ 内存 RunningApi.token，展示的 token 全 401。
  修法：活服务存在时优先返回 running.token）。
- **[P3] tiny_http 头部解析无行长/数量上限**（vendor/client.rs:80-102 buf 无界增长）——
  慢滴注超长 header 可无限占内存；35s deadline 只罩 body 不罩 header。
  修法：vendor patch 加 64KB/行、100 行上限，或补进 PATCHES.md 已知残留。
- **[P3] MemStore 与 TauriStore 列表排序语义漂移**（api.rs:59 插入序 vs db/tasks.rs:570
  `ORDER BY ord, rowid`）——MemStore 上通过的顺序断言不代表生产行为。
- **[P3] `due` 只限长不验格式**，任意字符串入库（handlers.rs:252-256/564-567）——
  due_notify 截止通知解析行为未定义。修法：非空 due 加格式白名单。
- **[P3] 请求不校验 Content-Type**（body.rs:34-101）——协议健壮性项，无安全后果
  （无 token 一律 401）。
- NEEDS-VERIFICATION：Windows 下 token 文件无 ACL 收紧（api_auth.rs:104-107 显式 no-op
  存根）——%APPDATA% 默认 ACL 可接受；便携模式（exe 落多用户可读目录）需实测。

### M12 模型元数据·配置·密钥·个人资料（meta/ + bot/config/ + profile.rs）

**[P2-1] update_config_file 路径静默剥掉未迁移的明文 legacy key（keyring 故障窗口内丢密钥）**（主会话已复核）
- 位置：bot/config/io.rs:520-532（update_config_file 只有 MCP 机密迁移）vs io.rs:415-418
  （add_allowed_dir 有 `strip_verified_keys` 防御纵深——grep 全仓仅此一处调用）+ io.rs:498-503
  （write_bot_config_file_locked 无条件把三槽置 None）
- 触发：启动迁移时 keyring 故障按「保留明文下次再试」（io.rs:585-586）→ 之后任何
  update_config_file 调用方（记忆整理 last_run 回写 / verified_vendors 落盘 /
  bot_clear_vendor_key）触发写盘 → 明文被剥但 keyring 里从未写入 → 密钥永久丢失。
- 修法：update_config_file 写盘前对三个 legacy 槽位复用 strip_verified_keys
  （零丢失语义：验证通过才剥，失败保留明文中止写）。

**[P2-2] 终稿自检侧调模型绕过 vendor 未验证门**（主会话已复核）
- 位置：bot_model_loop.rs:1273（主门）vs :1313-1330（selfcheck_http 构造无任何
  ensure_vendor_verified）
- 触发：selfcheckModelId 指向未验证厂商条目 → 主请求被拦，自检请求照发——
  真实 LLM 请求带 key 发往未验证 base_url。
- 修法：selfcheck_http 解析后补一道 ensure_vendor_verified，未验证回落主路径并审计。

**[P2-3] 厂商条目无 key 时回退全局主 key，可发往任意用户配置 base_url**（主会话已复核）
- 位置：bot/config/commands.rs:816-825（bot_test_connection 拿全局 key 探活）+
  bot/config/keyring.rs:630-642（read_llm_key 同款回退）；对照 types.rs:629-633 注释明确把
  跨厂商 key 借用定性为「凭据泄漏」并在 override 路径禁掉——active/探活路径保留回退。
- 触发：添加带 vendor 名、base_url 指向第三方的条目且未配该厂商 key → 全局主力 key 作为
  Bearer 发往该 base_url；对方对 GET /models 回任意 2xx 即进 verified_vendors 转正。
- 修法：带 vendor 条目无厂商 key 时探活改无鉴权、请求侧报 ApiKeyMissing；
  或厂商无 key 不得凭 2xx 写 verified_vendors。

P3：

- **[P3] 配置变更/key 写入清除/active 切换（含被 vendor 门拒切）零审计**
  （commands.rs:311-383/438-451/492-499，grep 证实）。
- **[P3] 损坏的 bot-config.json 无备份即回默认，下次保存覆盖**（io.rs:148-163 仅 eprintln；
  profile.rs:130-148 有备份+ERROR 审计可对齐）。
- **[P3] meta 模块文档漂移**：模块头称 8 个 meta_* 命令实际注册 3 个（lib.rs:770-772）；
  user_custom 写入路径本版本不存在（守卫属防御性在位）；同步只增不删，上游已删行永久残留。
- **[P3] max_tokens 注释陈旧**：「默认 8192」实为 32768（types.rs:135-137/341/503 vs schema.rs:235）。
- **[P3] bot_set_config keyring 先写、配置后写的半成功中间态**（commands.rs:320-344/377-383，
  io.rs:33-35 自认已转 B 类评估挂账）——未测试的 key 可能带着旧「已验证」绿点。
- **[P3-信息] verified_vendors 可经 bot_set_config 由前端整包注入**（commands.rs:224-261
  只删不加）——威胁模型内前端可信，记录 vendor 门强度边界。
- **[P3-澄清] profile.rs 已是预设头像 ID 制**：5MB/扩展名/data URL/路径穿越等旧攻击面整体
  不存在（AVATAR_IDS 双端 16 枚举+双侧校验，损坏备份+ERROR 审计在位）——旧检查项作废。

已查证安全：models.dev 同步 16MB 流式上限+单事务无半截+幂等；user_custom ON CONFLICT WHERE
守卫在位且无任何 DELETE；vendor 未验证门三道后端均在位且先于副作用（HTTP API 无 config 端点、
MCP 配置路径不触碰 active）；keyring NoEntry 良性区分（老账已清，classify 函数+测试）；
bot-config.json 三 key 字段 skip_serializing+写侧强制 None 双保险；schema 迁移幂等、未来版本
不降级、原子写 fsync；max_tokens/temperature/top_p 全边界钳制、NaN filter；PermMode 实为四态、
未知值落 Ask（安全偏严）；reasoning 只产结构化字段不拼 prompt；前端 botConfig.ts/modelMeta.ts
逐字段对齐、avatars.ts 双端 16 枚举有测试。

### M15 跨平台横切（全仓平台分支清单式专审）

**修复验证**：2026-10-21 的「Windows 停止阶梯空转却报已停」已修复未回退——
taskkill 三态区分（procs.rs:1076-1095）、停不掉 restore_state+`service.stop_failed` 审计
（services.rs:1667-1687）、MCP 停止后 pid_exists 复核不谎报（manager.rs:804-823）。
监听闸 Unsupported 三态：全部调用方按三态处理，前端确认消费 listeningSupported===false。

**[P2-1] macOS `UiWindow.focused` 恒 false——同契约两平台一真一假**（主会话已复核）
- 位置：ui/macos.rs:199（硬编码）vs ui/windows.rs:96（is_foreground 实测）
- 触发：macOS list_windows 永远报无前台窗口——模型在 Windows 能看到焦点窗口、
  macOS 看不到；bot_ui 据此标「（前台）」。修法：NSWorkspace.frontmostApplication
  比对 pid，或 Option<bool>+文档声明。

**[P2-2] Windows `UiNode.value` 恒 None——ui_inspect 在 Windows 读不到控件值且未声明**（主会话已复核）
- 位置：ui/windows.rs:278（CurrentValue 未读）vs ui/macos.rs:323（AXValue 实读）
- 触发：wait_for_element 的 query 搜索把 value 纳入匹配（bot_ui.rs:312-314）——
  Windows 上「按输入框内容找控件」整体失效，None 被当「值为空」。修法：读 UIA
  ValuePattern，或注释+工具描述声明 Windows 恒空。

P3：

- **[P3]** list_windows 空标题窗口策略两平台相反（Windows 跳过/macOS 保留且注释承诺的标注未兑现）；
- **[P3]** Windows Job Object 创建失败时内存/CPU 限额静默不生效无留痕（py/runtime.rs:108-134
  返回 None 无日志——与 unix S20「探测失败≠可用」纪律不对称）；
- **[P3]** 路径同一性全仓小写归一，Linux（大小写敏感 FS）上凭证表会过度归并
  （db/trace.rs:114 COLLATE NOCASE + bot_model_loop.rs:3370 to_lowercase）；
- **[P3][需验证]** 受管服务 stdout 按 UTF-8 lossy 解码，Windows GBK 输出服务 readyPattern
  永不匹配→被判就绪超时强杀、错误文案全乱码（services.rs:963-968；py 有 PYTHONUTF8=1，
  services 层无等价物）；
- **[P3]** Windows 便携包 OCR 依赖手工打包步骤，模型缺失时指引让用户跑 .sh（ocr.rs:183-184，
  Windows 无直接执行手段；packaging/ staging 目录与手册有版本落差）；
- **[P3][需验证]** Windows 长路径（>260）无全局兜底，bot_fs 剥 `\\?\` 前缀后失败报普通 IO 错误。

已查证安全：全部「能力不可用」分支要么显式 Err/Unsupported、要么注释声明（无静默装好新案例）；
pid_exists Windows 真实现；便携模式写探针+OnceLock 定版两平台一致；剪贴板失败分支对称清板；
CREATE_NO_WINDOW 统一常量+源码锁；单实例/自启/通知/托盘三平台统一且有防 cfg 门测试；
全仓无生产代码用 format! 拼 `/` 构造路径。

## 批 3 · 核心数据链（M1 任务卡 / M11 图谱标签 / M14 底座迁移）

### M1 任务卡与看板（db/tasks + exec_steps + 时间网格 + 浮窗/回收站/归档前端）

状态模型确认为三轴：`column`(todo/doing/done) + `archived` + `deleted_at`。

**[P2-1] 任务图谱铭牌 innerHTML 渲染任务标题——存储型 HTML 注入（CSP 拦住脚本执行，降级记录）**（主会话已复核 + CSP 分析）
- 位置：src/components/GraphPage/GraphCanvas.tsx:559-562（`off.innerHTML = ...${node.label}...`，
  全文件唯一 innerHTML）+ graph-build.ts:211（label = t.title，无转义）
- 触发：任务标题含 HTML → 打开图谱页。标题不可信来源：bot 写卡（提示注入可控标题）、
  `tasks_import` 外来信封（恶意共享 tasks.json）、HTTP API。
- **CSP 缓解**（tauri.conf.json:20 `script-src 'self' blob:` 无 unsafe-inline）：`onerror=`
  内联事件被 CSP 拦截，脚本执行面封死；connect-src 仅 ipc，无外传通道。剩余面＝标记注入 +
  style 注入（style-src 有 unsafe-inline，可伪装覆盖 UI 诱导点击）。
- 定级 P2（纵深防御仍应修——CSP 一旦回退即变 XSS→IPC）。修法：createElement+textContent 拼装。

**[P2-2] 软删不清调度字段：注释/SPEC/代码三方不一致**（主会话已复核）
- 位置：src/App.tsx:871-897——注释声称「软删必须清调度字段——否则任务躺在回收站里 schedule
  仍到点触发」，实际 patch 只写 `deletedAt`；SPEC.md §1 同承诺。
- 复核口径：注释里的理由已过时（tasks.schedule 列已随批 1 遗留清扫退役，无任何机制从回收站卡
  触发）；实际影响＝planStart/planEnd 随导出信封/HTTP API 外泄、恢复后旧排期复活。
  属「文档说的事实被代码否掉」同款契约漂移。修法：patch 补清 plan 字段或订正注释+SPEC。

**[P2-3] 详情浮窗截止时间无校验直写 due（与 TodoCard 双标，后端也不校验）**（主会话已复核）
- 位置：TaskDetailPanel.tsx:568（onChange 裸写）vs TodoCard.tsx:600-608（isValidDateTimeLocal
  守卫+注释）；后端 db/tasks.rs:858 `"due" => set_from(...)` 无格式校验（planStart/planEnd 有，明显不对称）
- 触发：datetime-local 打字过程中的部分值（"2026-10"）即入库 → due_notify 通知链解析行为未定义。
- 修法：前端套同一守卫 + 后端 due 补定长格式校验。

P3：

- **[P3] 数据层无状态机守卫**：绕过 UI 闸（bot 工具/task_patch 直调）可编辑、完成、排期
  回收站/归档卡（db/tasks.rs:708-739/791-963 无 deleted_at/archived 前置判断）。
- **[P3] 归档「恢复」把 completedAt 伪造成当前时刻**（TodoCard.tsx:504）——与 archiveRule.ts:50
  自己的注释口径矛盾；若为「重新计时」设计也应显式字段。
- **[P3] 「恢复待办」落点不一致**：详情面板回 todo、任务卡回 doing——今日到期卡会被
  applyTodayRule ≤60s 二次吸走（TaskDetailPanel.tsx:576 vs TodoCard.tsx:141-144）。
- **[P3] planEnd <= planStart 无跨字段校验**（db/tasks.rs:862-871 注释自认靠 UI 成对提交保证；
  bot/HTTP API 直写则网格隐身+假时长 60 分钟兜底）。
- **[P3] 绑定文件无存在性门槛、无悬空清理**（db/tasks.rs:208-224，metadata 失败照样绑定）。
- **[P3] 拖拽期间每次 pointermove 全页重渲染 + WeekGrid 无 memo 无虚拟化**
  （数百块以上/1000 卡为最坏档；TaskTimelinePage.tsx:163-268）。
- **[P3] 浮窗开着时卡被（他端）软删：浮窗不关、可继续编辑被删卡**（App.tsx:899-901 只兜硬删）。
- **[P3] 标题/备注/标签无长度钳制（前后端皆无）**（db/tasks.rs:928-943 仅拒空白；恶意导入信封可灌超长）。
- **[P3] 回收站恢复不级联**：母卡恢复后子卡仍留回收站（数据无损，纯 UX）。

已查证安全：TaskStatus wire 字节级兼容+脏值兜底读有测试锁；created_at 永不被 UPDATE 覆盖；
task_patch 白名单+未知/受保护键响亮拒绝+RMW 写冲突显式报错留审计；导出/导入路径闸门+64MB
bounded reader+事务；exec_steps 会话分槽+ExecGuard 防重入+三条路径复位头像；排期吸附/钳制
在前端且渲染期派生无写回路径（防死循环 guard 在位）；子任务进度纯渲染期派生；全部 SQL 参数化
（唯一 format! 是 IN 占位符个数）；tasks-updated 四源广播齐全；无 useMemo 依赖缺失死循环；
React 文本节点自动转义覆盖任务卡其余全部渲染面。

NEEDS-VERIFICATION：HTTP SSE overlay 收不到主窗 UI/bot 写（api.rs notify_change 仅 API 写路径
调用）——消费端有无轮询兜底待查（属 M10 联动面）。

### M11 图谱与标签（task_autotag / tag_similar / tags_consolidate / people + GraphPage）

2026-10-19 图谱审计修复点抽查 6 处（graphPrefs 迁移写确认后删、外观脏标、resumeForce 单一
唤醒点、declutter 预算等）**全部未回退**。

**[P2-1] 标签合并无事务、绕过 DB 写锁——中途失败产生不可回滚的半合并态**（主会话已复核）
- 位置：bot/tags_consolidate.rs:315-323（循环内逐条 UPDATE 自动提交）+ :333-340（history INSERT
  在全部 UPDATE 之后）；lib.rs:128-137 用裸 `open_db` 连接（对比 task_autotag 走 with_write_conn）
- 触发：合并 N 张卡时任一条 UPDATE 失败（典型：并发写者超 2s busy timeout）→ 前面的 UPDATE
  已落盘而回滚凭据（history 行）根本没写——「合并可完整撤销」在失败路径上不成立。
- 修法：整段搬进 with_write_conn，UPDATE 循环 + history INSERT 包进同一事务。

**[P2-2] `instr(tags, ?)` 子串匹配越界——主标签强加给从未拥有该别名的卡**（主会话已复核）
- 位置：bot/tags_consolidate.rs:268-277（SQL 对 JSON 原文做 instr 子串命中）+ :292-300
  （Rust 侧只按精确相等滤别名，然后无条件 push 主标签）
- 触发：簇别名「测试」+ 卡标签 `["集成测试"]` → instr 命中 → 别名删不掉（精确不等）→
  主标签照 push：从未带簇成员标签的卡被打上主标签（可撤销恢复但合并语义已错）。
- 修法：SQL 只粗筛，Rust 侧校验 old_tags 精确含任一别名才入 updates。

**[P2-3] 弹窗「+ 新建主标签…」未输入即提交 → 整簇并进字面量标签 `__new__`**（主会话已复核）
- 位置：TagConsolidateModal.tsx:94-97（value="__new__" 原样下传）+ :217-231（input 只在
  onChange 时替换 main）+ 后端 merge_tag_cluster 只查空串（tags_consolidate.rs:255）
- 触发：选中「新建」后不输入直接点合并。顺带：新建名无 trim、无 ≤12 字上限（与 autotag 口径不一）。
- 修法：提交前校验非占位值+trim 非空+长度钳制。

**[P2-4] 合并/撤销后不广播不刷新——归档页与图谱长期显示旧标签**（主会话已复核）
- 位置：lib.rs:131-146（merge/rollback 直接写库不 emit）对比 task_autotag.rs:210-219
  （双广播）；ArchivePage.tsx:219 只传 onClose——TagConsolidateModal 的 `onMerged` prop 是死参数。
- 修法：命令末尾广播 tasks-changed 或接上 onMerged 触发 reload。

P3：同义组键并查集写法使「组键=组内最小标签」注释不成立（GraphPage.tsx:200，显示层瑕疵）；
embed 不可用时整理扫描静默报「没有待合并簇」（tags_consolidate.rs:122-129，姊妹模块
tag_similar 会 audit+Err 口径不一）；模块头注释声称的 tag_merge 审计事件不存在（只有回滚有）；
信封成员卡每次导入抹掉已学到的头像（people.rs:75-78 None 直接覆盖，建议 COALESCE）；
tag_similar 词表无输入上限+缓存 512 条整体清空（>512 标签时每轮全量重嵌入 O(n²)）；
NEEDS-VERIFICATION：tags_consolidate_scan 同步命令体内跑阻塞式 ONNX 嵌入（是否卡界面取决于
Tauri v2 同步命令线程模型，建议对齐 tag_similar 的 spawn_blocking）。

已查证安全：autotag 幂等守卫链锁内重做；LLM 输出截断不整包拒；merge/rollback 影响域限定
归档/回收站卡（有测试）；autotag 与 merge 操作集合不相交无并发冲突；合并中新归档卡下轮自然
纳入；全部 SQL 参数化（动态部分只插占位符序号）；graphPrefs 损坏自愈；10-19 修复点无回退。

### M14 数据底座与迁移（db/lock + migrations + migration/ 十件 + paths + app_state + error + audit + bing + ocr）

模块定性：migration/ 是**桌面文件自动归档清理引擎**（cleanup-rules.json 规则表 + journal 崩溃
恢复 + 扫描整理），与「测试阶段不做数据迁移」决策无冲突；旧 tasks.schedule 数据按 114330be
退役口径随删列丢弃（符合决策，跨版本直升用户的旧定时数据无迁移属有意行为，备案）。

**[P2] with_write_conn 未置位 HOLDING_DB_WRITE 线程本地标记——debug 构建下自动打标回写必触发
debug_assert panic**（主会话已复核）
- 位置：db/lock.rs:48（直接 `DB_WRITE_LOCK.lock()` 不走 lock_db_write()）+ task_autotag.rs:203
  （with_write_conn 闭包内调 upsert_tasks）+ db/tasks.rs:294（`debug_assert!(holding_db_write())`）
- 触发：tauri dev / cargo test 下任务卡完成触发自动打标回写即 panic；真实互斥是满足的
  （锁确实持有），断言读的却是只有 lock_db_write() 才置位的标记。release 编译掉不受影响；
  测试未覆盖该回写分支所以 CI 不红。
- 修法：with_write_conn 临界区改用 lock_db_write()（一行，同时修好未来所有闭包内调契约函数的路径）。

P3：

- **[P3] journal_replay_pending 绕开 DB_WRITE_LOCK 写 journal**（migration/recovery.rs:75/118-159
  `_inner` 直调+锁外 open_db，失败 `.ok()` 吞掉——行保持 pending 无丢失，但纪律破口）。
- **[P3] frontend_event_report 对 detail 双重转义**（audit.rs:640 调用点预转义+中心路径再转义，
  `|` 倍增）。
- **[P3] bing 图片下载先全量进内存再做 8MB 检查**（bing.rs:154-165，建议 content_length 前置或
  bytes_stream）。
- **[P3] ocr 输入图片无大小/像素上限**（ocr.rs:70 std::fs::read 无上限 + :389 解码后 w×h×3
  驻内存——解码炸弹面）。
- **[P3] Windows OCR 引擎 OnceLock 缓存 Err：模型后装需重启**（ocr.rs:208，文档已声明，备案）。
- **[P3] app_state 模块头「已迁入 8 张」与 struct 实际 14 字段漂移**（app_state.rs:69 vs 83-120）。
- **[P3] bot_chat.rs:1710 锁标签文案名不副实**（锁的是 self.running）。

已查证安全：两把锁中毒恢复全仓一致；journal 单向状态机+孤儿清理+1000 条封顶有测试；migration
穿越三闸+TOCTOU 复检+staging 原子提交；CSV 导入 fail-closed 全链；open_db 幂等收敛（半截迁移
下轮补齐）；error.rs 24 code 三重测试锁死+From 转换不丢上下文+recoverable 分类有防回退测试；
audit 中心转义不可绕+error.log 镜像独立+防递归纪律；paths 便携判定四级兜底全测试；bing 缓存
原子提交标记+时钟回拨只少删；ocr 隐私红线入口双层+失败缓存留指引；app_state 兜底实例语义
验证成立（生产唯一注入点在 setup 内先于一切）。

---

### 10-21 审计 23 项修复逐条核验：**23/23 在位，0 削弱、0 回退、0 找不到**

逐项读到修复代码+测试锁（22 项有测试或扫描守卫；P1-10 为纯文档口径项无脚本锁、P2-1 的事件
发射仅间接锁 effective_output_cap——两者与报告口径一致无漂移）。P0-1 三点核验全在位：intents
零宽度三道闸（intent_router.rs:377）、确认窗展示 intents+mode（manage.rs:679）、三条装入入口
收敛 validate_created_meta。后续提交无削弱（2506fd32 是加固方向；4184513b 触碰文件但修复点原样在位）。

### M2/M6/M7 盲区抽查（全部 P3）

> 2026-10-24 复验：七条盲区 P3 已在「批 4 · M2/M6/M7 盲区 P3 复核」逐条复核是否仍成立
> （六条仍成立、Paused 超时一条已修在位），并新增同源点一条（compaction 预算漏算工具附图）。

- **[P3] Paused 技能 run 无超时**：等确认的用户永不响应 → 同名技能跨会话被无限期占位
  （bot_skills/runtime.rs:686 只对 Running 查 timeout；Paused 非终态不清扫）。
- **[P3] MCP 工具挂载只有单服务器/单 schema 上限，跨服务器总量无闸**（manager.rs:143-156
  128 条/台、mount.rs:61 8KB/条；mcp_tools_json_body 拼接处无总量钳制）。
- **[P3] Down 状态服务器懒重连无退避**：每次工具调用白付一次 10s 连接超时
  （manager.rs:261-297；建议 Down 记 last_attempt_ms 短路冷却）。
- **[P3] stdio 超长行截断对调用方不可见**：被截的恰是合法 JSON-RPC 响应时挂起的 call 只能等
  超时；截断只一次性 eprintln，不进 stderr 环形缓冲不进审计（stdio_transport.rs:91-97）。
- **[P3-low]** vars.rs params 值会被第二遍 step 变量扫描再展开一层（vars.rs:118-132）；
  Anthropic tool_result 的 is_error 靠文本启发式（bot_anthropic.rs:130-132，注释自述约束）；
  compaction 预算只统计字符串 content，工具附图的数组 content 不计入（compaction.rs:233-238）。

已查证无问题：Replan 链（钳制/失败兜底/停止优先）；bot_anthropic 并行 tool_calls/合并
tool_result/thinking 签名回传/stop_reason 四态；compaction 只压无 pending tool_call 的消息
（协议非法不可能）；intent_router 用 regex crate 线性引擎（ReDoS 结构性不可能）+编译失败哑弹
不拖垮整表；prompt_builder 槽位稳定排序+可变值钉末位保缓存；ChatGuard 会话级+Drop 兜底；
remote.rs 重定向逐跳过闸+钉 IP+64MB 双闸+落盘失败回滚；parse.rs 畸形 frontmatter 全兜底；
vars 单遍替换不递归；DSL 线性序列无环；MCP 撞名消歧进程级登记簿不漂移；secrets 水合失败
不缓存不阻断；并发调用走 rmcp 多路复用；stdio 读侧背压有界。

### M8/M9 盲区抽查（全部 P3）

> 2026-10-24 复验：四条盲区 P3 已在「批 6 · 批 6 盲区 P3 复核结论」逐条复核是否仍成立
> （四条仍成立，其中两条量级比记录时更大：跨服务器工具总量最坏 20MB/轮、Down 懒重连单次代价
> 从 10s 涨到 20s）。

- **[P3] 服务退出/停止后已挂的资源告警通知永不撤收**（services.rs:1784/1828-1840 撤收只在
  30s 巡检且仅 Running 态 → 孤儿 pending 占角标）。
- **[P3] 机器人总开关关闭时，到点作业每 30s 空转一轮 spawn+审计**（bot_scheduler.rs:413-419
  开关检查在推进水位之前 return；每天 2880 条 sched_job_skip 审计；panic 兜底路径 retry_at
  未清同样重触环）。
- **[P3] ask_user 超时/停止后 wfq:{qid} 通知永久 pending**（ask_engine.rs:413-441 Expired 只摘
  waiter 不 resolve 通知，也无后台清扫）。
- **[P3] 记忆抽取限频计数器纯进程内存**（重启即清零）+ **parse_extract 对非数组输出静默吞轮**
  （extract.rs:229-239 失败返回空 Vec 无审计，且限频窗口已被占）。
- **[P3] consolidate 矛盾裁决/合并只记数量**——被删条目内容不可追溯（consolidate.rs:333/362/
  491-494；记忆库无回收站）。
- **[P3] 主执行链（run_task_in_chat_with）无 panic 隔离**：panic → end_trace 不执行 → trace 行
  挂 running 最长 30 天（bot_chat.rs:2024 注释只保证「无早退分支」；子 agent 有 catch_unwind，
  主链没有）。
- **[P3·需验证] occurrence_after 跨日跳进用绝对 24h 加法**（bot_scheduler.rs:151-152/168-169）——
  DST 时区秋拨日 daily 同日双触发、春拨日晚 1h；国内无 DST 不触达，低优先国际化边界。

已查证无问题：watchdog 收尸三路齐全+FD_CLOEXEC 防误触发；资源告警采样/去重/撤收自洽；
scheduler Missed 判定+RMW 消费+防睡眠唤醒双跑；monthly 短月顺延两端一致；due_notify 24h
边界有测试；notifications 幂等键实际唯一；孤儿 queued/running 有启动清扫；六态跃迁表确认；
子 agent 预算原子计数+并发票据 RAII；ask 等待 24h 有界+预算回冲；U19 坏输出回退全 New；
consolidate 同事务顺序应用对已删行安全；embed 失败永久缓存是明示取舍；trace 16KB 字节钳
UTF-8 边界回退；trace_sink 批次事务+失败重连+DROPPED 计数。

## 批 4 · 聊天决策与记忆技能面（M2 聊天决策核心 + M6 记忆 + M7 技能）

三个只读子代理分头审 M2（bot_chat / bot_model_loop / intent_router）、M6（memory/ 全目录
9 件）、M7（bot_skills/ 全目录 11 件）。**P1 及以上由主会话逐条读码复核后才记为事实**
（本节 P1 三条、P2 六条均已复核）；P3 为子代理报告（均附代码行）。全程只读，未改一码。

行号说明：`bot_model_loop.rs` 与 `bot_orchestrator.rs` 在本轮被机械拆分（批 7/8），
行号会漂；发现项按「文件 + 函数名 + 关键代码片段」锚定。

### M2 聊天决策核心（bot_chat.rs / bot_model_loop.rs / intent_router.rs）

**[P1-1] 批量执行路由的 CJK 触发词是裸子串匹配——否定句/疑问句会自主开跑整批任务卡**
（主会话已复核）
- 位置：intent_router.rs:147-158（`is_chat_execute_trigger`）、:153（关键词表
  `["完成","执行","搞定","开干","做掉","go","run","do"]`）、:168-180（`keyword_hit`）、
  :170（CJK 分支 `user_cmd.contains(kw)`）；消费点 bot_chat.rs:974 → :325（`chat_execute_tasks`）
- 触发：UI 里选中 ≥1 张任务卡（前端把 `[已选任务]` 引用块拼在消息头）→ 用户正文**任意位置**
  出现「完成 / 执行 / 搞定 / 开干 / 做掉」之一即命中。例如「这两个什么时候**完成**？」、
  「先别**执行**这些」、「**执行**计划是什么」。
- 后果：命中即进 `PreStepRoute::ExecuteTasks` → 每张卡 `run_task_in_chat`（新建独立会话 +
  EXECUTE_SYSTEM_PROMPT + 50 轮工具循环），**全程无二次确认**。副作用工具（建卡/改卡/删卡/
  写盘）在用户只是提问时就被执行。
- 证据：同一函数的 ASCII 分支（go/run/do）要求「首词 + 右边界干净」，注释自陈
  「命中意味着把选中卡全部按自主执行跑一遍——误判代价是用户只想聊天却开跑了。**宁可漏触发**」；
  CJK 分支没有任何等价约束，等于把最常用的中文触发词放在最松的口径上。
- 修法：CJK 词也要求落在「命令位」（整段以该词开头 / 该词后紧跟动词性补语），排除
  「什么时候 / 是否 / 能否 / 不 / 别 / 先别 / 不用 / 无需 / 暂不」等前置修饰；或命中后加一次
  确认窗（ask 通道现成）。建议补回归用例 `trigger_rejects_negated_cjk_keywords`。

**[P2-1] 图片附件链路先整文件读进内存、再判大小**（主会话已复核）
- 位置：bot_chat.rs:457（`std::fs::read(&canon)`）→ :460（`bytes.len() > MAX_IMAGE_BYTES`）；
  工具回传侧 :498（`std::fs::read(p)`）→ :499（同样后判）
- 触发：白名单根目录（Desktop / Downloads / Documents / Pictures / AI_Gen_Files）内存在一个
  **图片扩展名的大文件**（几 GB 的全景 jpg、大视频改名 png），且消息里有一行以 `- ` 开头引用它
  ——`attach_images_in` 扫的是整条 content 的所有 `- ` 行，不限于附件块。
- 后果：`MAX_IMAGE_BYTES`（3MB）检查在 `fs::read` **之后** → 整个文件进堆；单条消息最多 4 张、
  最后 3 条消息都算 → 峰值可达 4×3×文件大小，直接压内存/OOM；读操作还在 `spawn_blocking` 里
  （bot_chat.rs:1167），会把 blocking 池线程占死。
- 修法：读前先 `fs::metadata(p).len()`，超限直接跳过（与已修的 bing content-length 前置、
  ocr 32MB 上限同款口径）。

**[P2-2] 终稿出口闸把「本提问问过自检」当「用户要 Y/N」**（主会话已复核）
- 位置：bot_model_loop.rs:3386-3388
  ```rust
  let asked_selfcheck = !*expecting_final_selfcheck
      && (last_selfcheck_round.is_some() || last_final_selfcheck_round.is_some());
  if asked_selfcheck && crate::prompts::parse_selfcheck_response(final_text.trim()).is_some() {
  ```
- 证据：`last_selfcheck_round` 只在 :2611 置位、**全程无清零**；`last_final_selfcheck_round`
  只在 :3470 置位、同样无清零 → 「问过自检」一旦成立就覆盖本提问余下全程，与「自检问句此刻
  是否还悬着」无关。
- 触发：用户明说「只回答 Y 或 N」，而模型这一轮调了变更类工具（complete_task / write_file）
  → 自检被注入并消费 → 模型再答一次 `Y` → 命中闸。
- 后果：闸把用户的合法答复当成「迟到的自检判定」，连推 2 轮追问；额度用尽后 `final_text`
  被**整体替换**成「（模型只回了自检判定「Y」，没有给出可读的结果）」——用户要的答案彻底丢失，
  另外白烧 2~3 次 LLM 调用。该闸的存在理由正是修「用户只看到一个 Y、真正的报告没产出」的事故，
  它自己又制造了同一类症状。
- 修法：闸的触发条件绑定「自检提问仍悬在当前轮」而非「本提问问过自检」；补一条用户显式
  要求 Y/N 的豁免。

**[P3-1] SSE 读侧两条错误出口不配对 `skill_finish` → 活动技能 run 停在 Running** —— **已修复**
- 位置：bot_model_loop.rs `read_sse_stream` 的两条 `Err`（流式读取超时 / 流式读取失败）
  直接穿出循环体（`?`），**未调** `(deps.skill_finish)(false, …)`；对照 `handle_error_response`
  的 Fatal 路径、`stream_round` 的流内错误路径、`request_round` 的网络失败路径**都调了**。
- 触发：模型调了 `use_skill`（会话技能 run 置 Running）→ SSE idle 超时（120s 无数据）或连接中断。
- 后果：该会话技能 run 停在非终态 Running（UI 仍显示在跑、步数停在中间），直到技能超时清扫
  或下次 `start_skill` 才被标 Failed。
- 修复：读侧失败改为返回 `StreamFailure`（错误本体 + 「失败前是否已向前端吐过内容」），
  交主循环统一收口——还没吐过内容就重发本轮（`MAX_STREAM_RETRIES=1`）；吐过了、或重发仍失败，
  就收尾技能（`skill_finish(false, "流式响应中断")`）后上抛，不再留悬空 run。
  复现用例：`tests/llm_integration/stream_failure.rs` 三条（重发本轮 / 已吐过内容不重发 /
  放弃重发时收尾技能）。

**[P3-2] intent 准入闸第三道（探针）可被整体跳过 → 过宽模式能落盘并进路由表**
- 位置：intent_router.rs `content_free_probe`（FILLERS 12 个字符）+ `check_intent_pattern`
  （探针为 None 时**直接跳过第三道闸**）；调用点 bot_skills/manage.rs:554。
- 触发：模式字面字符集覆盖全部 12 个 FILLERS → 探针 None → 第三道闸不执行；而第二道闸只要求
  「≥2 连续字面量字符」，一个可选字面量组即可满足。例：`(?i)(日报)?[xyz09\-_~@#中日\s\S]*`
  ——字面量段「日报)」3 字符过闸二，字符集覆盖 12 填充符 → 探针 None → 闸三跳过；该正则可匹配空串。
- 后果：装上即进进程级路由表，此后每条消息都被劫持进该技能；`mode=auto` 时整段 DSL 无 LLM
  复核直接执行。**缓解**：两条安装入口都有人工确认窗且展示 intents，用户有机会看出异常正则。
- 子代理用 Python 忠实复刻 `scan_pattern` + `content_free_probe`（纯函数、不写仓库文件）验证：
  对该模式判定 = 放行；对照组 `(?s)(润色|修订)?.*` 正确被拒。

**[P3-3] 早退出口不合并 task_refs → 停止 / 熔断路径返回重复引用**
- 位置：bot_model_loop.rs `run_model_loop_core`：只有主出口前有
  `collected_refs = merge_task_refs_dedup(collected_refs);`，其余 16 处
  `return Ok((…, collected_refs))` 早退出口（已停止 ×6、轮数/单轮熔断、
  Skill AwaitConfirm/Finish/Fail/Terminate ×5）直接返回未去重的 `collected_refs`。
- 触发：同一轮里模型对同一张卡产生两次相同 id 的引用（create_task 后又 edit_task 同 id），
  随后用户 `/stop` 或撞熔断 → 前端渲染出重复按钮；同一场景走正常主出口则不重复。

**[P3-4] 非 2xx 响应体与 SSE 单行长度无上限（与 MCP stdio 读侧不对称）**
- 位置：bot_model_loop.rs `request_round`（`let text = r.text().await.unwrap_or_default();`
  错误体整包进内存）、`drain_sse_lines`（按 `\n` 切行，单行无长度上限 → `byte_buf` 可无限增长）。
- 对照：MCP stdio 读侧有超长行截断（账本已记 P3），LLM SSE 读侧没有同一纪律。

**[P3-5] `RouteAction::ExecuteTasks` 是不可达分支**
- 位置：bot_chat.rs:982 `RouteAction::ExecuteTasks(_) | RouteAction::PassThrough => None,`。
- 事实：`route_user_input_for` 只可能返回 `Skill` / `PassThrough`；`ExecuteTasks` 只由独立的
  `is_chat_execute_trigger` 产出 → 该 match 臂永不可达。无功能影响，但枚举契约读起来像
  「路由表统一产出三类」，实为两条路。

**[P3-6] 汇报轮也会走 L0 规范化 → 与用户无关的审计事件 + 末条文本与库里不一致**
- 位置：bot_chat.rs:860 `if let Some(n) = normalize_user_input(&mut messages) { … }`。
- 触发：`TurnOrigin::Report` 且库里的最后一条是 user 消息（助手回复尚未落库的窗口期）。
- 后果：库里那条不会被改（改的是内存副本），但会多出一条 `user.input_normalized` 审计
  （用户没做过任何输入操作），且模型看到的末条与库里的原文不一致。
### M6 记忆（memory/ 全目录 9 件）

**[P1-2] 记忆导入会静默删除既有记忆，且报告仍按「新增 N 条」计数——注释承诺的「只增不删」不成立**
（主会话已复核）
- 位置：memory/panel.rs:537（注释「只增不删，走既有语义去重合并；容量满按 skipped 计」）、
  :404-408（`import_items` 注释「只增不删。skipped 口径：容量满拒写」）、
  :429-439（逐条计数）、memory/store.rs:314-341（`evict_if_needed`）、
  :496（`insert_new` 每次插入都先调淘汰）、:418-433（合并分支）
- 触发：设置页「记忆库 → 导入」选一个条目数超过「容量 − 现有条数」的 JSON（容量默认 500）。
  例：库里有 300 条，导入 400 条 → 第 201 条起 `count >= capacity` 成立 → `evict_if_needed`
  每次先 `DELETE` 淘汰分最低的一条再 INSERT → 原有 300 条中约 199 条被删（淘汰分最低者优先是
  早期导入的 importance=3 条目），且报告 `report.inserted` 逐条 +1 → 前端 toast 显示
  「新增 400 条、跳过 0 条」，**只字未提删了 199 条**。
- 后果：① 用户记忆被无提示删除（记忆库无回收站）；② 成功计数是「插入次数」而非净增；
  ③ 文档三处口径（「只增不删」「容量满按 skipped 计」「容量满拒写 → skipped」）与代码三处矛盾
  ——`RejectedFull`（→skipped）只在**全库都是受保护条目**时才发生，正常路径一律走淘汰。
- 修法：导入路径显式声明容量策略——要么「导入前临时抬高容量、导入完按淘汰分收敛并把被淘汰
  条数写进报告」，要么「容量满即 skipped（不淘汰）」并让报告与 toast 如实显示；同时把
  panel.rs:537 / :404-408 的注释改成与实现一致。

**[P1-3] `mem_import` 在持有全局 DB 写锁期间跑 ONNX 预嵌入 + 最多 5 万次插入——注释承诺的「锁外预嵌入」与代码相反**
（主会话已复核）
- 位置：memory/panel.rs:537-538（注释「解析、输入校验与预嵌入在锁外（文件 IO + ONNX 推理不占
  DB 写锁临界区）」）、:555（`let _g = lock_db();`）、:557（锁内才调 `import_core`）、
  :451-469（`import_core` 内 `parse_import` + `prepare_import_items`）、
  :392（`prepare_import_items` 第 392 行调 `reembed(content)` = `embed::embed_text`）
- 触发：导入一个**不含 `embedding` 字段**的 JSON → N 次 ONNX 推理 + N 次 `load_all`
  （≤500 行 × 2KB 向量）全在 `DB_WRITE_LOCK` 临界区内（JSON 解析也在锁内）。
- 后果：`DB_WRITE_LOCK` 是全进程单写锁（db/lock.rs:43-58），任务卡、trace、聊天历史、
  调度器全部排队 → N=2000 即锁住写锁数十秒~数分钟，N=50000（上限 panel.rs:535）可达数十分钟，
  表现为「应用数据层整体卡死」。
- 修法：把 `parse_import` + `prepare_import_items`（含预嵌入）整体移到取锁之前（与
  `mem_list`/`mem_update` 的「嵌入在持锁前算」纪律对齐，panel.rs:231/:253 就是正确写法），
  锁内只留事务落库。
- 说明：ONNX 单条耗时（按 bge-small-zh 短文本估 10–30ms）是量级估计、未实机测量；但
  「推理与全表反序列化发生在全局写锁内」这一事实与耗时无关。

**[P2-3] 语义去重合并会用抽取条目的「空 tags」抹掉既有记忆的 key / 来源 / 重要性**（主会话已复核）
- 位置：memory/store.rs:418-433（合并分支 `UPDATE mem_items SET content = ?1, tags = ?2,
  importance = ?3, source = ?4 …`，`?2` = 新条目的 `tags.join(",")`）、
  memory/extract.rs:522-546（`apply_insert` 构造 `tags: Vec::new()`、`source:
  "model_inferred"`）、store.rs:296-299（`is_protected = importance>=5 && source=="user_stated"`）、
  store.rs:534-538（`find_by_key_tag` 按 `tags[0]` 精确匹配）
- 触发：`autoExtract=auto`（或 confirm 档用户点「收下」）抽出一条与既有记忆余弦 ≥ 0.92 的
  画像/偏好条目；既有那条是模型用 `remember_fact` 写的带 key 条目（key=「口味」、value=「不吃辣」、
  importance=5、source=user_stated）。
- 后果：合并把既有行整体改成抽取条目的值——`tags` 变空（「口味」这个 key 消失）、`source` 变
  `model_inferred`、`importance` 变抽取给的（常见 2）。于是：① 之后 `remember_fact` 用同 key 再写
  找不到旧行 → 改走「新插入」→ 同 key 语义变成堆积重复；② 原本受保护（不淘汰）的用户口述条目
  变成可淘汰条目；③ 注入块里这条从「用户口述」变成 `[推断]` 徽标。全过程只有
  `memory.extract_adjudicated` 的 new/update/skip 计数，删除与覆盖内容不可追溯。
- 对照证据：memory/tests.rs:39-70 明确锁定了「合并时 tags 被新值覆盖」是设计（且新条目带 key
  时语义可接受）；**未被覆盖的是「新条目无 tags」这一路**——抽取路径正是这一路。
- 修法：合并时对空 tags 做保护（`tags = CASE WHEN ?2='' THEN tags ELSE ?2 END`）；
  `importance`/`source` 合并策略改为「取更权威的一方」而非无条件覆盖新值。

**[P2-4] 注入预算对「用户画像与偏好」段不生效——`## 记忆` 块可无上限膨胀**（主会话已复核）
- 位置：memory/mod.rs:32-33（`MEMORY_BUDGET_CHARS = 4_000`）、:290-306（裁剪循环：
  `trimmable_from = if pinned.is_empty() {0} else {1}` → **画像段永不裁**；其余段裁空后
  `find` 返回 None → break）、memory/rank.rs:191-195（`pinned` = 全部
  `importance>=4 && kind∈{profile,preference}`，**无条数上限**）、mod.rs:6（模块头「预算 4000 字」
  写成不变契约）
- 触发：累积 ≥6 条 `importance≥4` 的 profile/preference 记忆（单条 content 上限 800 字），
  6×800 = 4800 > 4000。
- 后果：注入块突破预算（设置页 `memoryTuning.injectionBudgetChars` 调到 1000 也一样被突破），
  且该块作为 **system 角色**消息注入每一轮聊天与每次任务执行 → token 成本与首字延迟随画像
  条数线性上涨。当前裁剪循环**没有测试覆盖**（memory/tests.rs 搜不到 `format_memory_block_with`
  预算用例）。
- 修法：画像段改为「按预算裁剪 + 超预算时给一条显式提示」，或给 pinned 加条数上限，并补一条
  预算回归用例。

**[P3-7] 两条记忆写入路径不持 `DB_WRITE_LOCK`**
- ① bot_skills/runtime.rs:564-602（`sink_skill_failure_lesson`：`open_db` 后直接
  `record_lesson_core` 写 `mem_items`，全程无写锁）；对照正确写法 memory/mod.rs:751-754
  （`auto_lesson_on_task_failure` 取了锁）。
- ② memory/extract.rs:706-735（`notify_memory_proposals`：`notif_insert` 是写操作却裸写）；
  对照 ask_engine.rs:355-358「通知落库持 DB 写锁，不裸写」。
- 触发：主窗/其它模块正持有长写事务时 → SQLite busy timeout 后 `SQLITE_BUSY` → 教训丢失
  （只留一条 `skill.lesson_sink_failed` 审计）/ 提案通知丢失。

**[P3-8] 导入文件里带逗号的标签会把整批导入中止，并报成「存储故障」**
- 位置：memory/panel.rs:367-402（`prepare_import_items` 校验了 content 长度、kind、source，
  **没校验 tags 是否含逗号**）、memory/store.rs:392-394（对含逗号的 tags 直接 `Err`）、
  panel.rs:429-439（`Err` → 整批回滚，文案「导入中止（存储故障…）」）。
- 后果：本应计入 `skipped` 的**输入级**违规被当成存储故障 → 整份文件一条都导不进去；错误文案
  误导用户去查存储/磁盘。

**[P3-9] 导入文件自带的 `embedding` 不做维度/有限性校验就落库**
- 位置：memory/panel.rs:392（`it.embedding.clone().or_else(|| reembed(content))` → 文件里的向量
  **原样使用**）、memory/store.rs:109-129（`blob_to_embedding` 读侧才拒非 512 维 / NaN）、
  panel.rs:190-196（`stats_core.with_embedding` 注释自称「写入侧只落合法向量」——被导入路径打破）。
- 后果：① 该行读回时向量判 None → 静默退化为关键词模式（面板仍显示「有向量」）；
  ② 超长向量原样入 blob，之后每次 `load_all` 都要把这坨字节读一遍。

### M7 技能（bot_skills/ 全目录 11 件）

**[P2-5] 会话档技能（`scope: session`）的边界只落在路由层：技能清单与 `use_skill` 启动层都没有会话边界**
（主会话已复核）
- 位置：bot_skills/manage.rs:286-290（`build_skill_block` 只按 `enabled` 过滤，**无 scope 过滤**）、
  :296-323（`build_skill_block_for` 同款）、:11-25（`SkillInfo` **没有 scope 字段**）、
  bot_skills/runtime.rs:21-40（`preflight` 只查 `enabled` + 危险词黑名单，**不查
  `session_scoped`**）、:127-234（`start_skill` 无会话归属校验）、manage.rs:761-763（设计意图
  原文：「工具来源的技能只在本会话路由（本会话立即生效、跨会话不劫持、重启即失效）」）
- 触发：会话 A 中模型经用户确认建了技能 `my-helper`（落盘 skills/my-helper/SKILL.md，
  frontmatter 被强制写 `scope: session`）。此后：① 会话 B（同一应用、另一个会话，甚至重启
  应用之后）的系统提示词里 `build_skill_block_for` 仍会列出它（清单是全局扫描磁盘）；
  ② 模型在会话 B 调 `use_skill("my-helper")` → `start_skill` 读盘加载 → `preflight` 放行 →
  **技能在会话 B 正常执行**（auto 模式下 DSL 直接跑，不经 LLM 复核）。系统提示词第 55 条还写着
  「技能以当轮『已安装技能』清单为准」，等于官方承认清单是权威——而清单里就有它。
- 后果：批 4a 的「跨会话不劫持 / 重启失效」只在**自动路由**一条路上成立；模型自建技能仍可跨会话
  执行（其正文由模型写，只经 description/intents 黑名单与宽度闸，正文本身不被扫描），且设置页
  也无法区分哪些是会话档（`SkillInfo` 无 scope 字段，用户看不出、也删不掉这个语义）。
- 修法：`SkillInfo` 加 `scope` 字段并在清单层按当前会话过滤；`start_skill` 增加会话归属校验
  （`meta.session_scoped` 时 run 的归属会话必须等于当前会话，否则拒启动并提示「该技能仅在创建
  它的会话内可用」）。

**[P2-6] `install_skill` 的确认窗不展示 intents / mode——第三方技能可自带「宽触发词 + auto 模式」**
（主会话已复核）
- 位置：bot_skills/remote.rs:652-668（确认文案只有「技能名 + 来源 + 文件数」）
  vs bot_skills/manage.rs:774-784（`create_skill` 的确认窗展示 intents 与 mode，并明说
  「mode = auto = 命中后不经 LLM 复核直接执行技能步骤」）；remote.rs:645-650（准入核只查
  description/intents 黑名单 + intents 宽度）、:683（装完 `rebuild_intent_routes` → 进全局
  路由表）、bot_skills/parse.rs:307-313（`risk_level: low` 且未显式声明 mode → **强制 auto**）
- 触发：用户（或模型）请求安装一个第三方 SKILL.md，frontmatter 写 `risk_level: low`
  （→ 模式被判为 auto）、`intents` 为一个宽泛但能过三道宽度闸的模式。用户在确认窗只看到
  「安装技能「x」（来源：GitHub …），共 8 个文件」→ 点同意。
- 后果：装上后该技能进入**全局**路由表；此后任何用户消息命中 intents 即被路由进该技能，
  auto 模式直接按 DSL 顺序执行工具调用，**不经过 LLM 复核、也不弹用户确认**。触发面与执行方式
  都没让用户看见——与 2026-10-21 审计 P0-1「确认窗展示 intents+mode」在 `create_skill` 上的
  修法口径不一致，第三方入口漏了同一道闸。
- 修法：`install_skill` 确认窗复用 `create_skill` 的口径，展示 `intents`（逐条）与 `mode`
  （auto 时显式标注「命中后不经 LLM 复核直接执行」）；对 `mode=auto` 且 intents 过宽的第三方技能
  额外要求一次显式勾选。

**[P3-10] `skill_terminate_all` 持技能注册表锁时写审计日志**
- 位置：bot_skills/scheduler.rs:185-204（第 201 行 `audit_log_hook` 在 `runs` 守卫存活期间调用）；
  对照本模块自己的纪律：runtime.rs:204-206 / :385-396 / :501-503（注释原文「持锁跨外部调用
  = 慢 IO 串行化全部写者 + 钩子重入 registry 即死锁」）。
- 后果：写盘 IO 期间所有技能状态操作被串行阻塞；不构成死锁，属纪律破口。

**[P3-11] 用户经设置页导入的技能若带 `scope: session`，装上后不产生任何路由条目且无提示**
- 位置：bot_skills/manage.rs:250-274（`intent_rules_from_dirs` 对所有目录来源统一过滤
  `meta.session_scoped`）、:470-521（`skills_import` 原样落盘，不剥离/不改写 scope；
  返回文案只有技能名）。
- 后果：技能出现在列表里、可以被 `use_skill` 手动调起，但**永远不会自动触发**，且导入成功提示
  里没有任何「该技能为会话档、不参与自动路由」的字样 → 用户以为装坏了。
### M2/M6/M7 盲区 P3 复核（批 3 遗留 7 条，逐条复核是否仍成立）

**[P3-12] 记忆抽取限频计数器纯进程内存（重启即清零）** —— 盲区 P3 复核：**仍成立**
- 位置：memory/extract.rs:184-215（`OnceLock<Mutex<HashMap<String,i64>>>`，无落盘、无跨进程
  恢复；`throttle_check_set` 仍是纯内存 check-and-set）。

**[P3-13] `parse_extract` 对非数组输出静默吞轮（失败返回空 Vec 无审计，且限频窗口已被占）**
—— 盲区 P3 复核：**仍成立**
- 位置：memory/extract.rs:229-263（非数组 → `Vec::new()`，无审计）、:572-575
  （`facts.is_empty()` → 直接 `Ok(())` 返回，无事件）、:284-286（限频窗口在 spawn 前已被占住）。

**[P3-14] `consolidate` 矛盾裁决/合并只记数量——被删条目内容不可追溯（记忆库无回收站）**
—— 盲区 P3 复核：**仍成立**（E 组明确未修，本次确认在位）
- 位置：memory/consolidate.rs:333（`report.merged += delete_by_ids(...)`）、:362、:456-458、
  :491-494（审计事件只带 merged/distilled/contradictions 三个计数）；store.rs:585-594 的
  `delete_by_ids` 无影子表、无回收站。

**[P3-15] `vars.rs` params 值会被第二遍 step 变量扫描再展开一层** —— 盲区 P3 复核：**仍成立**
- 位置：bot_skills/vars.rs:118-132（`substitute_vars_with_params` 先替换 `${params.*}`，随后调
  `substitute_vars_inner` 再跑一遍步骤变量扫描 → params 值内含 `${stepN.result}` 会被第二遍展开）。

**[P3-16] Anthropic tool_result 的 is_error 靠文本启发式** —— 盲区 P3 复核：**仍成立**
- 位置：bot_anthropic.rs:126-132（注释自述：本层拿不到 ToolStatus，只能走文本分类，判据是
  `crate::audit::tool_call_failed("", &text)`）。

**[P3-17] compaction 预算只统计字符串 content，工具附图的数组 content 不计入** ——
盲区 P3 复核：**仍成立，且新增同源点**
- 原点：bot/compaction.rs:233-238（`count_chars` 只 `filter_map(|m| m["content"].as_str())`）；
  **新增同源点**：bot_model_loop.rs `compact_before_round` 算 `msgs_chars` 同样只统计字符串
  content → 工具附图的数组 content 两处都不计入，压缩分级（70%/85%/100%）会低估真实体量。

**[P3-18] Paused 技能 run 无超时** —— 盲区 P3 复核：**已修复，当前仍在位**
- 修复合入 `b1ac8348`（`git show --stat` 确认）。当前代码：bot_skills/runtime.rs:53-71
  （`sweep_expired_pauses` 同时扫 Paused 与 Running）、:76-94（`skill_conflict_with_existing`
  放过已过期 Paused）、:190（`start_skill` 启动清扫）、:466-483（`confirm_state` 过期不再恢复、
  标 Failed）、:765-773（`advance_skill` Paused 超时 → Fail）；复现用例 runtime.rs:1306-1344
  （`sweep_expired_pauses_marks_failed_and_frees_slot` 等 4 条）。后续提交未削弱该逻辑。
### 批 4 已查证无问题（新增条目，子代理实读确认）

**M2**：批量执行引用块解析（标记在消息头、块体到首个非 `- ` 行为止、破损行跳过不中断块；
正文里伪造的 `- id=evil，标题=删库` 不会被收进块）；附件白名单判定（canonical 双向比较、
软链解析后落点必须在白名单根内、读 canonical 落点消 check-then-read 软链掉包窗、单消息 ≤4 张
≤3MB）；历史截断即摘要（`truncate_split_point` 保证最后一条永远保留；摘要失败/空白退回纯丢弃）；
ChatGuard / ExecGuard（Drop 兜底释放含 panic 展开、按注入实例隔离、无会话 id 不加锁）；任务执行
入口读库失败上抛不吞成「卡不存在」、完成/归档/软删三态前置校验齐、trace begin/end 成对且
begin 之后无早退分支；汇报轮「已吸收」时机（只在回复**落库成功后**才 `mark_digested`，有源码
接线守卫单测）；幻觉守卫判据组合（变更类按 `ToolStatus::is_success()` 判定、文件路径两道、点名
判据有排除集、不可核验工具统一让开）；数值出处判定（`number_sourced` 支持字面命中与四舍五入
等价、`result_numbers_in` 排除贴边字符/列表序号/计数单位/估算词/序数）；`merge_stream_usage`
按字段取最后一次非零值（累计型 usage 不重复计入）；工具调用累积双上限（MAX_TOOL_CALL_INDEX=64
+ 单条 arguments 1MB 上限）；请求降级链（stream_options 只在错误体**点名该字段**时降级、
Anthropic thinking 只在同时出现 thinking+signature 时补救且只补救一次、429/5xx 退避重试、
上下文超长砍史后**整体重建 body**）；砍史的协议安全（切口不落在 tool 分组中间、
`reindex_after_shed` 同步修正下标表）；熔断上限单点钳制 `fuse_cap_for` + 漂移守卫单测；
批 5 审计写侧统一后的解析端（全仓 Rust 与前端**没有**任何解析端假设老 free-form 行格式的地方）；
`intent_router` 用 regex crate 线性引擎（无 ReDoS 面）、非法模式被跳过且不拖垮整表。

**M6**：记忆表不在 `db/` 下，`mem_items`/`mem_pending` 由模块自建、幂等 `IF NOT EXISTS`；注入契约
在系统提示词里有明确口径（记忆是背景参考不是本轮指令）；全部记忆 SQL 参数化（唯一 `format!`
的 `col` 取自固定数组）；`MemoryTuning::clamped` 对 8 个字段全钳制、decay/cosine 对 NaN/Inf 收敛；
提示词资产使用正确（抽取/裁决/整理分别用 `crate::prompts::*` 三个常量，无就地拼串）；嵌入降级
纪律（`OnceLock<Result<Engine,String>>` 缓存失败原因永久返回 None，不 panic；`blob_to_embedding`
拒非 512 维与 NaN/Inf → 异形向量不会互相命中污染去重与召回）；抽取裁决的幻觉/双写防护
（`parse_adjudication` 要求 `existing_id` 与候选 id 精确相等、坏输出整体回退 New、同候选被多条
update 认领时降级 Skip；`update_by_id` 返回 0 行按「并发已删」计 failed 不当成功；整批在事务内）；
整理指令应用的事务与幻觉防护（`apply_ops_with` 全程在 `conn.transaction()` 内、逐 id 主键点查、
目标行 0 行则整条跳过不误删）；注入取数的零命中兜底（无向量且零关键词重合 → 0 分）；
面板路径锁纪律基本正确（`mem_list`/`mem_update` 的嵌入在取锁前算、`mem_export` 的序列化与原子写
在锁外、`mem_delete` 走 `with_write_conn`）；待确认队列的原子性（整批入队+删行同事务、
入库 Err 不删队列行）。

**M7**：技能名与路径白名单（`SKILL_NAME_CHARS_OK` 在三处一致、目录名与 frontmatter 名**双校验**）；
`create_skill` 附件闸齐备（绝对路径/`..` 拒、逐组件字符集校验、禁 SKILL.md 覆盖、单文件 200KB /
10 个 / 合计 1MB 三道上限、排他占位 + 失败回滚不留半份目录）；导入/远程安装的拷贝安全
（`copy_dir_all` 跳符号链接、拒 FIFO/socket/device、递归深度 ≤16；`copy_skill_tree` 三闸；
解包走 `bot_archive::Limits::unpack()` 挡住 zip slip / 软链 / 解压炸弹）；远程下载的 SSRF 闸
逐跳生效（禁自动重定向、每跳重新过 `check_public_url` + `http_client_pinned` 钉 IP、64MB 双检）；
会话档路由的会话可见性过滤成立（`add_session_route` + `visible_under_session`，
intent_router.rs:783-796 有用例）；状态机决策纯函数化且口径单源（`advance_skill` /
`advance_dsl` 不写状态；`skill_timed_out` 是步数熔断/状态机/暂停清扫/冲突判定的唯一超时口径）；
回滚窗口的原子工具放行有 Drop 兜底（`RollbackRestoreGuard` 在 panic/提前 return 时复原 Failed
终态）；DSL 步骤号连续性校验与变量替换单遍不递归；`allowed-tools` 声明是逐步骤硬校验
（违反走回滚 + FailedButRecoverable）；内置技能物化的幂等与退役清理成立。

### 批 4 未覆盖 / 存疑（明说，不假装审过）

- 未跑 cargo、未做实机复验：所有结论基于读码；P3-2 的绕过配方用 Python 忠实复刻纯函数验证
  （未改动仓库任何文件）。
- `bot_model_loop.rs` 行号会漂（批 7 机械拆分在审期间进行），按函数名 + 代码片段复核。
- 未审：bot_skills/scheduler.rs（除 P3-4 与相关接缝）、bot_orchestrator.rs、bot_plan.rs、
  memory/* 的测试块（memory/tests.rs 831 行、memory/consolidate/tests.rs 395 行未逐行读）、
  bot/registry.rs 其余部分、前端全量（仅读了与命令契约相关的片段）。
- 技能状态机只看了局部（runtime.rs:42-121 + 731-811），P3-1 的「停在 Running」后果依赖这套
  清扫逻辑，按读到的代码推导，未实机验证 UI 表现。
- **存疑（需产品侧口径）**：M6 P1-1 的定级有第二种读法——若产品本意就是「记忆库恒定 ≤500 条、
  导入即收敛」，则缺陷不在淘汰行为本身，而在「报告不区分插入数与净增数、且 UI 与注释都说
  『只增不删』」。本账本按「静默删用户数据 + 成功计数错误 + 文档三处背离」定为 P1；若拍板
  「导入即替换库」是既定产品语义，这条可降为 P2（改报告与文案即可）。
- **一条观察（不计为发现）**：memory::store_lock()（mod.rs:117-119）走 `lock_or_recover`，
  **不置** `HOLDING_DB_WRITE` 线程标记；而 M14 的修法把 `lock_db_write()` 定为唯一置位入口。
  当前记忆临界区内没有调用带 `debug_assert!(holding_db_write())` 的函数，故暂无 debug panic
  路径；但这是同款「拿锁方式与标记脱节」的形状，建议将来 memory 侧若要在锁内调契约函数，
  先把 `store_lock()` 收敛到 `lock_db_write()`。

---

## 批 5 · 前端专审（M13）

### 渲染与注入面

全仓危险通道扫描：dangerouslySetInnerHTML/outerHTML/insertAdjacentHTML/document.write/
iframe/srcDoc/window.open/eval/new Function/DOMParser **全仓零命中**；innerHTML 唯一命中即
M1 已记的 GraphCanvas 铭牌。href/src 动态通道 12 处逐一定性全部安全（打包静态资产/16 张
头像注册表/本地 lobehub SVG/models.dev 的 logo_url 已不消费/必应 data URL 后端硬编码前缀）。

- **[P3·需验证] http(s) 链接只拦左键，中键/Cmd+点击（auxclick）可能让 webview 本体导航外域**
  （MarkdownText.tsx:98、RichText.tsx:37 只有 onClick preventDefault，无 onAuxClick；Rust 侧无
  on_navigation 处理器。最坏＝整页跳外域钓鱼，非代码执行；是否真能导航取决于 Tauri v2 对
  webview 跨源导航默认策略。修法：气泡层全局 auxclick preventDefault 或 Rust 侧导航拦截转 opener）。
- **[P3] 必应大图外跳 URL 未做域名白名单**（bing.rs:146-151 `starts_with("http")` 即原样采用，
  API 响应被劫持时点卡片跳任意 URL；点击门控+走系统浏览器，定 P3。修法：后端校验 host 为
  bing.com 系）。

已查证安全：MarkdownText 用 react-markdown 且无 rehype-raw（原始 HTML 默认转义）+自定义
链接组件协议白名单（非 http/https 一律 span 化，javascript:/data:/ipc:// 全中性化）+react-markdown
默认 urlTransform 双保险；AI 注入外域图片被 CSP img-src 拦截；ChatPanel 流式中走纯文本正则
链接化（半截标签不进解析器）、完成后才全量 markdown；工具入参/结果全走 `<pre>` 文本节点；
CommandPalette/通知/trace/日志/diff 全文本节点；ErrorDialogHost/ErrorBoundary 文本节点。

### 状态管理与错误链路

- **[P3] applyRemoteRows 绕过 mutating 串行链**：与 tasks-updated 处理器的 await 窗口交错可
  短暂丢行（App.tsx:834-842 vs :526-549；窗口极窄且 events 双通道自愈；建议并入 chain）。
- **[P3] applyRemoteRows 套规则但不落盘**，与 tasks-updated 处理器行为不对称（规则变更只进
  UI 态，重启回退，等每分钟定时器补救；App.tsx:836-841）。
- **[P3] taskEq 用 JSON.stringify 全串比较**，对键序敏感可产生一次性假变更落盘
  （storage.ts:8-10；多余写+无意义 updatedAt 推进。修法：逐字段比较或键排序）。
- **[P3] homeKpi 对畸形 due 返回 NaN，onTimeRate 分母含永不可能进分子的项**（homeKpi.ts:107-113/
  182-186；仅脏数据触发。修法：Number.isFinite 过滤分母）。
- **[P3] archiveRule 启动拉取配置失败按默认 7 天落盘归档**，与后端实际配置（如 30 天）漂移
  造成过早归档且方向不可逆（archiveRule.ts:34-41 + App.tsx:600-607。修法：loaded 前不应用规则）。
- **[P3] botRun.emit 失败完全静默**（botRun.ts:6-8 `.catch(() => {})`，点 🤖 无任何反馈）。
- **[P3-观察] useInlineEdit 隐式契约**：onCommit 后父组件必须退出编辑态，否则二次提交被吞
  （useInlineEdit.ts:53-74；现两处调用点均满足，建议补注释）。
- NEEDS-VERIFICATION：archiveRule 前端 `>=` 与后端 resolve_archive_after_days 的恰 N 天边界
  方向一致性未逐行核对；bubbleStyle.ts:120 注释声称的 storage 事件跨窗口同步不存在（单窗口
  架构下无影响，注释与现实不符）。

已查证安全：globalErrorReport 双层熔断（前端 20 条/会话+Rust 32 签名去重）+上报失败不递归+
StrictMode 防双装；errorHandler 24 code 三重漂移防线+recoverable 分流；useConfirm 走 OS 原生
ask 并发天然排队+fail-closed；ConfirmMap busyRef 防双发+条件清理不误杀新请求；useTauriListen
cancelled flag 全覆盖（含 StrictMode 迟到 resolve 自注销）；App.tsx 无乐观更新（先落盘再
setState 天然免回滚）+mutate 串行链+stamped map 防基线落后+source 协议五源对齐有测试锁；
storage.ts 实为 SQLite 封装+排序变更豁免刷戳；homeKpi 并查集口径与「连通分量≥2」一致+
口径单源成立；types.ts 抽查 12 项契约镜像全对齐（含 expectedUpdatedAt 单向下行、TaskOut
wire 形状锁）。

---

## 批 6 · 外部接入与服务进程面（M8 MCP 外部服务器 + M9 受管服务进程）

两个只读子代理分头审 M8（bot/mcp/ 全 9 件）与 M9（bot/services.rs 2391 行 +
bot/services/watchdog.rs + bot/procs.rs）。**P1 及以上由主会话逐条读码复核后才记为事实**
（本节 P1 一条、P2 一条均已复核）；P3 为子代理报告（均附代码行）。全程只读，未改一码。

按增量口径：以 10-21 的 MCP 子审结论与批 3「M8/M9 盲区抽查」为底稿，不重审已核绿的清单。

### [P1-1] 工具发现路径与工具调用路径不互斥——同一台 MCP 服务器会被并发拉起两个子进程，
且发现路径会覆盖在途调用的连接槽（主会话已复核）

- 位置：bot/mcp/manager.rs:460-497（`discover_tools`，**全程不取 `call_lock`**）、
  :463（唯一的活连接守卫 `matches!(self.slots().get(&cfg.id), Some(Slot::Connected { .. }))`）、
  :490-497（发现结束**无条件** `slots().insert(cfg.id, Slot::Idle{..})`）、
  :325-361（`ensure_connected`，:337 `slots().remove(&cfg.id)` **先摘槽再 spawn**）、
  :381-387（`call_lock` 是 per-server 闸）、:1084-1099（`call_tool`，:1090-1091 是全仓**唯一**
  取 `call_lock` 的地方）、:390-396（`tools_known`，槽位不存在 → 返 false）
- 触发（按需启停下这个窗口很宽）：
  1. 会话 A 调某 MCP 工具 → `call_tool` → `ensure_connected` 在 spawn **之前**把槽位摘掉
     （:337），此后 10s（握手）+ 10s（tools/list，最长 20s）内该 server id 在槽表里**不存在**；
  2. 此刻另一条主 agent 会话（定时作业与用户聊天并行）或设置页点「查看工具」
     （commands.rs:367 → `discover_tools_for` → `discover_tools`）进入 `ensure_tools_known`/
     `discover_tools` → `tools_known` 判 false → **对同一 server id 再 spawn 一个子进程**。
- 后果：
  1. 同一 server id **两个子进程并存**（npx/node 双份内存——正是本次按需启停改造要消除的代价）；
  2. 发现路径结束时 `slots().insert(Slot::Idle)` **覆盖**在途调用的 `Slot::Connected` → 面板状态
     显示 idle、`connected_children()` 取不到该 pid（`mcp_status` 的 pid/RSS/CPU 行凭空消失），
     而进程实际在跑；
  3. `retire`（manager.rs:569-595）发现槽位不是同指纹的 Connected → 直接 return → **停止阶梯被
     跳过**，子进程由 `CappedChildProcess::drop` 的 `start_kill()` 强杀（stdio_transport.rs:195-197），
     无 SIGINT 优雅退出、无脱离进程组的子孙回收；
  4. 退出时 `shutdown()` 只收槽位里的 Connected（manager.rs:1151-1163），被覆盖的活连接不在表内
     → 只能靠 Arc drop 的 SIGKILL 兜底，组外子孙可能留孤儿。
- 证据：作者已知「覆盖槽位会把在途调用的连接抽走」——:461-462 的注释原文如此，但守卫只挡了
  **Connected 态**这一种；「槽位不存在」（spawn 在途）这一种没挡。
- 修法：`discover_tools` 与 `call_tool` 共用同一把 per-server `call_lock`；给 `ensure_connected`
  的 spawn 期间加一个「占位」态（如 `Slot::Connecting`），使 `tools_known` 在此期间不再误判为
  「未发现」；`discover_tools` 插入 Idle 前复核槽位是否已被别的路径换成 Connected。
- 复验建议：补一条复现用例（两会话并发调同一 MCP server，断言 `ps` 里只出现一个子进程 +
  槽位最终为 Connected/Idle 而非错态）。

### [P2-1] 受管服务的两道权限开关由 0644 flag 文件承载，且开关变更零审计（主会话已复核）

- 位置：bot/services.rs:2173-2188（`set_enabled` → :2179 `std::fs::write(&path, b"1")`
  → 默认 0644）、:2205-2207（`expose_net_flag_path` = `services-expose-net.flag`）、
  :2215-2233（`set_expose_net_allowed`）；闸位消费点 bot/tools.rs:2044（闸 1）与
  bot/services.rs:756 / :2210（`allow_non_loopback` → `rule_of` services.rs:372、421）
- 触发：同机同用户的任意进程（或任何能写 `{data_dir}/runtime/flags/` 的东西）创建
  `services-enabled.flag` / `services-expose-net.flag`；两处都不记录「谁在什么时候开的」。
- 后果：
  1. 绕过「允许机器人启动本地服务」的用户显式同意 → 模型可直接 `start_service` 起 node/npx/
     python3 长驻进程（白名单内即可）；
  2. 更重的是 **L1 暴露闸被静默解除**：`services-expose-net.flag` 存在即 `allow_non_loopback=true`，
     端口闸的 L1 判据直接让路 → 受管服务可绑 `0.0.0.0` 暴露到局域网/公网（这正是
     MANAGED-SERVICE-DESIGN-2026-10-21.md §L1 要守的边界）；
  3. 事后从 bot.log **查不到**这个暴露是用户开的还是别的进程开的（`service_set_enabled` /
     `service_set_expose_net_allowed` 全程无审计事件）。
- 证据与对照：同仓 `src/ui/mod.rs:139-158` 的同款能力开关已按 2026-10-23 修复口径处理——
  **建即 0600 + 收紧盘上旧 flag + 写 `ui_automation_toggle` 审计**（模块注释自陈「0644 时同机的
  其它进程照样能创建这个文件，把闸门顶开」）。服务侧这两道开关既没权限收紧也没留痕。
- 修法：`set_enabled` / `set_expose_net_allowed` 复用 `ui::write_flag_file` 同款 0600 写入 +
  `set_permissions` 收紧盘上旧 flag；变更时写 `service.enable_toggle` /
  `service.expose_net_toggle` 审计（带 enabled + os_user）。

### [P3-1] MCP 工具调用把配置读取的线程 join 失败折叠成「MCP 服务器已被删除」

- 位置：bot/mcp/mount.rs:254-260（`spawn_blocking(...).await.unwrap_or_default()`）
  + :274-277（`None` → 「已被删除」）。
- 触发：`load_config` 在 blocking 线程里 panic（配置解析路径 panic）→ join 返回 Err →
  `unwrap_or_default()` → 空列表 → 命中 `None` 分支。
- 后果：诊断归因错（模型被告知「服务器已被删除」，会转去让用户重新添加，而真相是读配置崩了）；
  同时违反仓库「不吞错误」纪律（10-21 已记同一条，本轮复核仍在位）。
- 修法：`.await` 的 `Err` 分支显式上抛「MCP 配置读取失败（内部错误）」，不要落到「已删除」分支。

### [P3-2] MCP 结果上限的注释与真实口径不符（30,000 ≠ fetch_url 的 65,536）

- 位置：bot/mcp/mount.rs:55-56、:372（「fetch_url 同口径」）；对照 bot/registry.rs:2085
  （`fetch_url` 的 `max_output_bytes: 65536`）。
- 后果：文档误导（当前方向是**更严**，无即时安全后果）。

### [P3-3] 受管服务的 stdout/stderr 日志落在系统 temp 目录、文件 0644、目录权限不收紧

- 位置：bot/services.rs:986-995（`services_root`/`make_service_dir`）、:1002-1008（`open_append`
  → 默认 0644）。
- 触发：Linux 多用户机器上 `std::env::temp_dir()` = `/tmp`（共享）→ `wmessage-services/svc_xxx/
  stdout.log` 以 0644 创建 → 同机其他用户可读；`wmessage-services` 目录本身也可被他人**预先放置
  同名符号链接**（/tmp 粘滞位挡不住「新建」）。
- 后果：dev server 的 stdout（可能含启动横幅里的 token / 内网地址）泄露给同机其他用户。
  同类：M4 在账本已记「TMPDIR 未指向本次运行目录」的 P3，服务层是同款。

### [P3-4] `mcp_server_save` 的「keyring 先写、配置后写」半成功中间态

- 位置：bot/mcp/commands.rs:93-113（闭包内 `upsert_in_config` → `store_server_secrets` →
  `write_bot_config_file_locked`）。
- 触发：keyring 写成功、随后写配置失败（磁盘满 / 权限 / 占用）→ 命令报错、配置没落盘，但 keyring
  里的 env/headers **已是新值** → 下次 `load_config` 用旧配置 + 新机密水合。
- 后果：配置与机密不同步（用户看到「保存失败」，token 却已换）；方向比 M12 那条轻（不丢密钥），
  但同样是半成功态且无留痕。

### 批 6 盲区 P3 复核结论（批 3「M8/M9 盲区抽查」中属本批的四条）

| 账本条目 | 结论 | 依据 |
|---|---|---|
| **MCP 工具挂载只有单服务器/单 schema 上限，跨服务器总量无闸** | **仍成立**（且量级比账本记录时更大） | manager.rs:203 `MAX_TOOLS_PER_SERVER=128`/台、mount.rs:61 `MAX_SCHEMA_BYTES=8KB`/条、config.rs:24 `MAX_MCP_SERVERS=20`；mount.rs:421-430 `build_tools_json_body` 拼接处**无总量钳制** → 最坏 20×128=2560 条 × 8KB ≈ **20MB/轮请求体**。按需启停后盘上快照让这批工具一次性挂进清单，触发面比 10-22 时更宽 |
| **Down 状态服务器懒重连无退避** | **仍成立**（单次代价变大） | manager.rs:325 `ensure_connected` 不查冷却；`discovery_retry_at` 冷却只在 manager.rs:448 的**发现**路径被读，**调用路径完全不看** → 一次坏服务器调用白付 `spawn_service` 的握手 10s + tools/list 10s = 最多 **20s**（账本记的 10s 是旧口径） |
| **stdio 超长行截断对调用方不可见** | **仍成立** | stdio_transport.rs:86-98：超限只 `eprintln!` 一次，**不进 stderr 环形缓冲、不进审计**；被截行不是合法 JSON-RPC → rmcp 逐行忽略 → 在途 call 只能等 `cfg.effective_timeout_secs()`（config.rs:124-128，钳 5..600s）超时，最坏挂 **10 分钟** |
| **服务退出/停止后已挂的资源告警通知永不撤收** | **仍成立** | services.rs:1859 `sample_resources` 只处理 `state == Running` 的条目，非 Running 直接 `continue`；唯一的按服务撤收在**同一循环体内**（services.rs:1903-1915）→ 服务 stop/自行退出后，其 pending 资源告警永远留在通知中心占角标（合计口径那条会撤，走 services.rs:1950-1959 的独立分支） |

### 批 6 已查证无问题（新增条目，子代理实读确认）

**M8**：名字登记簿不漂移（`by_owner`/`by_name` 双向绑定、已分配名不再释放给别的归属、64 字节截断
后缀哈希防撞；`find_mounted` 先放锁再取挂载表，**无 std Mutex 嵌套死锁**；`assign` 递增时撞真实
工具名继续递增）；跨服务器函数名全局唯一（有单测 mount.rs:481-494）；白名单双清单一致
（保存口 `config.rs:159` `contains` + spawn 逐臂字面量 `manager.rs:636-655`，parity 由
`stdio_allowlist_and_spawn_match_stay_in_sync` 锁死——源码层无「变量→进程名」数据流）；
HTTP 分支 SSRF 三闸齐全（字面量闸 + `bot_web::check_public_url` DNS 级闸 + `resolve_to_addrs`
钉 IP + `Policy::none()` 禁重定向；`http_branch_goes_through_the_two_gate_client` 以源码文本守卫
「不得退回裸字面量闸」）；URL 字面量闸覆盖面完整（localhost 家族/尾点/纯数字与 0x 形态 IPv4/
.local/IPv4-mapped/IPv4-compatible/6to4/Teredo/NAT64/文档段逐条拒，配单测）；机密链路
（keyring 写后读回比对、blob 2048 字节上下限对称、降级单文件 0600 + RMW 互斥、掩码还原
`resolve_masked`、空机密=清条目、**hydrate 读失败不缓存**）；入参本地校验（非法 JSON 与合法但
非对象都在本地拒，不静默透传 Null）；超预算审计真落 bot.log（事件名与内置侧同名、带 server
字段区分来源）；工具快照（指纹不一致即失效、坏文件 fail-soft、tmp+rename 原子写、删除服务器时
连带清快照）；子进程生命周期（传输 Drop 兜底 `start_kill`、`close()` 先关 stdin 再 3s 优雅退出
再强杀、MCP 停止走与受管服务**同一套**信号阶梯且停止后再核 `pid_exists` 不谎报）；内置条目
（删除被拦、启动只补缺失项、发布构建不回落到构建机路径）。

**M9**：进程树公共模块（pid 0/1 拒当树根避免宿主自杀、PID 复用核启动标识后才杀、整组杀核组长
身份、脱离进程组的子孙按 ppid 树补杀、快照为空时「宁可赌一次」的口径有注释）；看门狗三路收尸
+ FD_CLOEXEC（unix 管道 EOF 两端 CLOEXEC、助手不继承写端、Windows Job Object、`disarm` 在
「被停止」与「自行退出」两条路径都调）；停止口径（停不掉就 `restore_state` + `service.stop_failed`
审计，**不写 Exited**；就绪失败「先判定后停止」，归因不串）；端口闸三态（Unsupported（没判）≠ Ok
（判了没事）；采样失败不判且**保留集动态部分不清空**）；2026-10-24 批 4b 采集金丝雀在位且未被削弱
（API 开着且采样自称成功却看不见 API_PORT → 判本轮失明（不判）+ 保留集不动 + 转折审计
`service.listening_canary_failed`；API 关闭时无基准不误报）；资源告警口径自洽（阈值/持续判定/
去重/撤收在一个纯函数里；合计口径在采不到时返 `None`（没判）而不是 0）。

### 批 6 未覆盖 / 存疑（明说，不假装审过）

- 未执行任何构建或测试（按任务要求只读）。所有结论基于源码阅读 + 与账本/设计文档交叉核对；
  **P1-1 的并发竞态结论是代码路径推演，未做实机复现**——建议按上文的复现用例口径落地。
- 未审：bot/tools.rs 的服务工具层闸序（只按调用点读了闸 1/3/4）、py/commands.rs 的
  `py-enabled.flag`、前端 McpPanel.tsx / ServicesPanel.tsx 渲染面、api_server::ApiState、
  notifications.rs 完整语义（只按 `resolve_service_alert` 三元组撤收读了接口与单测）。
- **交叉提示（不在本批范围）**：`py-enabled.flag`（py/commands.rs:20-38）与本批报出的服务侧
  两道开关是**同款写法**（0644 + 无审计），属 M4/M12 已审模块，本批未展开，建议下一批一并收口。
- **存疑 A**：MCP stdio 子进程**继承宿主全量环境变量**（manager.rs:656-657 只 `envs` 增不删、
  无 `env_clear`）——用户 shell 里的密钥会进第三方 MCP 服务器。服务侧 `start_service` 的 schema
  **明确写了**「服务继承宿主环境变量（含其中的密钥）」，**MCP 侧没有等价声明**，10-21 的 MCP
  子审也未记这条。触发面是「用户自己配的第三方服务器」，故只作观察提请拍板（是否补声明 /
  补白名单），未按缺陷定级。
- **存疑 B**：`mcp_server_save` 半成功中间态（P3-4）要不要回滚机密，属口径选择，需产品侧定。
- **存疑 C**：跨服务器工具总量闸（盲区 P3-1）加不加总量闸，会牺牲外部工具可见性——需拍板，
  子代理只给了量级换算。
- Windows 路径按源码审（`taskkill` 三态、Job Object、`lsof`/`/proc` 缺失导致的 Unsupported 语义），
  **无 Windows 实机验证**。

---

## 修复批次 · 2026-10-23（M5 的 5 条 P1）

按「一项逻辑变更一个 commit」的基线口径逐条修：每条先落复现用例（红）→ 修 → 变异
复验（改坏 → 用例红 → 改回 → 绿）→ 单独提交。

| 编号 | 缺陷 | 修法 | 提交 |
|---|---|---|---|
| P1-1 | 危险关键词大小写敏感，英文按钮全绕过 | 匹配先归一成小写；两字母以内的短词（"ok"）按整词匹配，"Outlook" 不再误命中 | `80487196` |
| P1-2 | `ui_type` 不中和换行，文本带 `\n` 即触发发送 | 带换行的文本直接拒绝（不静默剥除），schema 同步写明这条约束 | `4559a905` |
| P1-3 | 敏感判定不含 AXSecureTextField / UIA IsPassword | 判定加角色（AXSecureTextField）与平台标志（UIA IsPassword → `UiNode.is_password`） | `4c698b08` |
| P1-4 | Windows 坐标点击把像素当 0–65535 归一化坐标 | 按虚拟桌面换算（配 MOUSEEVENTF_VIRTUALDESK），桌面外的点不出手 | `25fdcd9b` |
| P1-5 | 注入不要求目标窗口在前台 | 注入前校验目标窗口在前台，判不出来也不注入（fail-closed）；Windows 坐标兜底同样收紧 | `d61707df` |

**这批没动的**：M5 的 6 条 P2（窗口 id 复用 TOCTOU、闸门快照与实际动作二次解析不一致、
无标题按钮绕确认、注入无全局互斥、开关变更无审计+flag 权限、授权模型是否产品化）；
「授权模型（应用白名单）是否要产品化」仍待拍方向。其余分组（B 凭据面 / C 数据一致性 /
D 跨平台契约 / E P3 择优 / F 待定性）未开批次。

### 修复批次 · 2026-10-23（B 凭据面 3 条 P2）

同样口径：先落复现用例（红）→ 修 → 变异复验 → 单独提交。

| 编号 | 缺陷 | 修法 | 提交 |
|---|---|---|---|
| M12 P2-1 | 写盘把盘上残留的旧明文密钥抹掉（keyring 故障窗口内永久丢密钥） | 写盘前先把旧明文安置进凭据存储；安置不了就**中止写盘**（三个 key 字段是 skip_serializing 的，写盘必抹） | `5c5b6e93` |
| M12 P2-2 | 终稿自检的侧调请求绕过厂商未验证门 | 侧调也过 `ensure_vendor_verified`；未验证 → 回落主路径内的自检 + 审计 | `4e8f5581` |
| M12 P2-3 | 厂商条目没配 key 时借用全局主 key（可发往任意用户配置的 base_url） | 请求侧不再借用（报缺 key）、探活不带鉴权、没带鉴权的 2xx 不转正 | `48e96dad` |

**这批没动的**：C 数据一致性组（M11 四条 / M1 三条 / M14 一行）、D 跨平台契约（M15 两条）、
E P3 择优、F 待定性 11 处。

## 修复批次 · 2026-10-23（C 数据一致性 8 条 + D 跨平台契约 2 条 + E P3 择优 7 条）

同样口径：先落复现用例（红）→ 修 → 变异复验（改坏 → 用例红 → 改回 → 绿）→ 单独提交。

### C 数据一致性

| 编号 | 缺陷 | 修法 | 提交 |
|---|---|---|---|
| M14 P2 | `with_write_conn` 裸锁 `DB_WRITE_LOCK` 但不置 `HOLDING_DB_WRITE` 线程标记 → 闭包内调 `upsert_tasks` 的断言在 debug 构建必触发 | 临界区改走 `db::lock_db_write()`（唯一会置位的入口） | `2a4d5fd1` |
| M11 P2-1 | 合并逐条 UPDATE 自动提交、history 行最后才写 → 中途失败留半合并态（改写了标签但没有回滚凭据） | 改写搬进 `conn.transaction()`；回滚同样包事务；签名改 `&mut Connection`；命令层走 `with_write_conn` | `bac7f803` |
| M11 P2-2 | 别名用 SQL `instr(tags, ?)` 子串粗筛（「集成测试」含「测试」也命中），Rust 侧却无条件加主标签 → 无关卡被改写 | 精确判定移到 Rust 侧（精确相等才加主标签） | `bac7f803` |
| M11 P2-3 | 「新建主标签」占位值 `__new__` 原样下传 → 建出一个真叫 `__new__` 的标签 | 后端加 `NEW_MAIN_PLACEHOLDER`，拒占位值/空白/超长（长度上限取系统口径 `MAX_TAG_LEN`=30，不是 autotag 的 12） | `8572b5d3` |
| M11 P2-4 | 合并/撤销后不广播 → 归档页与图谱长期显示旧标签；模块头一直承诺的 `tag_merge` 审计事件没写 | 命令层广播 `tasks-updated`（行级）+ `tasks-changed`；补 `tag_merge` 审计事件；`RollbackResult` 补回写的任务 id 供前端刷新 | `8572b5d3` |
| M11 附带 | 「新建主标签」输入框根本打不出名字（输入第一个字符 → `main` 变该字符 → 输入框当场消失） | 「新建」改成独立 `creating` 状态，不再拿 `main` 兼任两义 | `48b53c9b` |
| M1 P2-1 | 图谱铭牌用 `innerHTML` 拼不可信任务标题（bot 写卡 / 外来信封 / HTTP API）→ 标题里的尖括号被当标记解析 | 改 `createElement` + `textContent` 逐段拼装（类名不变，样式不动）；另加一条常驻守卫扫全仓 10 个危险口子必须零命中 | `c23bc5d7` |
| M1 P2-2 | 软删不清调度字段：注释 / SPEC / 代码三方不一致 | **订正口径、不改代码**：`tasks.schedule` 两列已随遗留清扫退役，定时作业走独立表；`planStart/planEnd` 是网格位置不是触发器；导出是全量备份不是泄漏面；可逆软删应保留位置 | `e7f73523` |
| M1 P2-3 | 截止时间无格式校验 → 打字半截值落库，解析失败被当成「无截止」，通知静默不发 | 详情浮窗套 `isValidDateTimeLocal`（与卡片同一守卫）；后端 `task_patch` 写口加 `validate_due`（与 `due_notify` 同口径，只认 `YYYY-MM-DD` 与 `YYYY-MM-DDTHH:mm`）；不挂 `upsert_tasks`（否则旧存量行重存会被拒） | `92769b45` |

### D 跨平台契约 + A 组 M5 余下 P2（合成一个提交——`ui/mod.rs` 改签名会让中间态编译不过，拆不开）

| 编号 | 缺陷 | 修法 | 提交 |
|---|---|---|---|
| M5 P2-1 | macOS 窗口 id 复用 TOCTOU：闸门审的是 A 窗，动作打在同 id 的 B 窗 | 动作前复核窗口身份（pid + 标题），变了就拒绝并让模型重取 id | `1c9ea780` |
| M5 P2-2 | 闸门审的控件 ≠ 实际动的控件（界面在 inspect 与 click 之间变了） | 新增 `NodeIdentity`（标题/角色/位置），两平台后端动作前逐项比对，对不上就拒绝 | `1c9ea780` |
| M5 P2-3 | 无标题按钮（纸飞机、垃圾桶这类图标按钮）绕危险闸——判不出危险就默认放行 | fail-closed：无标题一律要确认 | `1c9ea780` |
| M5 P2-4 | 注入无全局互斥 → 并发注入互相踩 | `INJECT_LOCK` 全局互斥（截图留锁外，不占闸） | `1c9ea780` |
| M5 P2-5 | 总开关变更零留痕（事后查不到「UI 自动化是什么时候被谁打开的」）；开关文件建即 0644 | 开关变更写 `ui_automation_toggle` 审计（带 `enabled` 与当前系统用户）；unix 建即 0600 并收紧盘上旧 flag | `1c9ea780` |
| M15 P2-1 | macOS `focused` 恒 false（没实测前台窗口） | 按 NSWorkspace 实测前台窗口，与注入闸同一口径 | `1c9ea780` |
| M15 P2-2 | Windows `UiNode.value` 恒 None | 读 UIA `CurrentValue`（密码框跳过读值，避免读出明文） | `1c9ea780` |

### E P3 择优（7 条）

| 编号 | 缺陷 | 修法 | 提交 |
|---|---|---|---|
| 僵尸 app_runs 行 | 失败路径不回写 → 状态永远停在 running；进程被杀则永远僵尸 | 失败路径统一回写终态 + 陈旧 running 行清扫（阈值 2h）+ 启动即扫 | `ce3faead` |
| api-token 空残留 | 空 token 文件占着路径，`create_new` 恒撞 AlreadyExists → `api_start` 恒失败 | 空文件改走原子覆写自愈（tmp + rename，不留明文残渣） | `afe7f47b` |
| bing 大图 | 整个响应体先进内存才判大小 | 先查 `content-length`，再流式读边读边计数 | `992d7c5c` |
| OCR 解码炸弹 | 输入无字节上限、无像素上限 | 字节上限 32MB + 像素预算 20MP（解码前只读文件头） | `e07b7a0d` |
| Paused 技能占位 | 等确认的技能无限期占着同名技能，别人起不来 | Paused 纳入超时（口径与熔断、状态机统一）+ 启动清扫 + 冲突判定放行过期项 | `b1ac8348` |
| 调度器空转 | 机器人总开关关着时，到点作业每 30s 派发一次、每次都被拒，一天几千条噪声 | 纯函数 `classify_job` 判 `Held`：不派发、水位不推进（保住「恢复后补跑」语义）、不写跳过审计；panic 后清 `retry_at` | `633e7111` |
| taskEq 键序敏感 | `JSON.stringify` 全串比较，键序不同即判成变更（多余写 + 无意义推进 updatedAt） | 改键序无关的稳定序列化再比 | `b2ad135a` |

**这批没动的**：A 组第 6 条 P2（授权模型 / 应用白名单是否产品化）——它不是缺陷，是要你拍方向；
F 组 11 处 NEEDS-VERIFICATION 仍等你给口径。

**本批的验证**：Rust 单测 1867 通过 / 前端 1011 通过 / clippy 干净 / 全量门禁
（nextest + vitest + 29 条对拍）全绿；架构文档的行数标注同步更新（模块树变了，行数跟着变）。

---

## 汇总与拍板清单

### 总计

**P0 0 条 / P1 9 条 / P2 27 条 / P3 95 条 / 待定性（NEEDS-VERIFICATION）15 处**；
P1 分布：M5×5（桌面 UI 自动化，10-23 已修）、M2×1（批量执行 CJK 触发词）、M6×2（记忆导入静默
删除 / 导入持写锁跑 ONNX）、M8×1（MCP 发现与调用路径不互斥）；P2 分布：M5×6、M12×3、M11×4、
M1×3、M15×2、M4×1、M14×1、M2×2、M6×2、M7×2、M9×1。

> 2026-10-24 补审两批后（批 4 = M2/M6/M7、批 6 = M8/M9）：P1 +4 / P2 +7 / P3 +22 / 待定性 +4。
> 六批齐了，15 模块 × 审计状态全部落账。批 4/批 6 的 P1 及以上均由主会话逐条读码复核后才记为
> 事实；P3 为子代理报告（均附代码行）。

验证性结论：2026-10-21 的 23 项修复 **23/23 在位零回退**；2026-10-19 图谱轮修复抽查在位；
架构文档 §6.4 对拍 157=157；基线全绿（nextest 1981 / vitest 985 / tests-audit 29 / clippy 干净 /
machete 干净）。整体判断：**没有 P0**——安全边界（本地 API、keyring、vendor 门、MCP SSRF 闸、
压缩炸弹闸、脚本路径收窄）经逐条攻击性验证全部成立；本轮主病灶是**闸门判定材料可绕**
（UI 自动化）与**凭据/数据一致性边角**（keyring 故障窗口、合并无事务），多为上轮审计
→「七链」之外的模块的首轮深审收益。

> 补审两批（批 4/批 6）后的第二判断：主病灶从「闸门判定材料可绕」转向**跨层承诺与实现背离**
> 与**并发/锁纪律破口**——M2 的 CJK 触发词裸子串匹配（代码自陈「宁可漏触发」却对中文放最松口径）、
> M6 的导入「只增不删」注释与容量淘汰实现相反、M8 的发现与调用两条路径不互斥（同一台 MCP 服务器
> 两个子进程）、M9 的权限开关 0644 且零审计（同仓 ui/ 同款能力已有 0600 + 审计的成例）。
> 这几条的共同形状是**「注释/设计文档写着一套，代码跑着另一套」**，靠读实现读不出来。


修复推进到 2026-10-23 收口：P1 五条 + P2 **十九条落地**（M5 的「授权模型」一条不是缺陷、
是要产品侧拍方向，另计），P3 择优七条落地。修完的基线：**Rust 单测 1867 通过 / 前端 1011 通过
/ tests-audit 29 通过 / clippy 干净 / 全量门禁全绿**。剩 F 组 11 处待你给口径。

> 账实更正（2026-10-23 逻辑一致性审计发现）：本行原先写「P2 二十条全部落地」，与修复表
> 对不上——修复表实列 18 条 + 授权模型另计 = 19 条，M4 的 P2-1（validate_app_id 放行 "."，
> 彻底删除可抹掉整个程序根）当时未修。该条已于审计修复批单独修掉（id 白名单
> `[A-Za-z0-9_-]+` + 复现用例），见 docs/logic-consistency-audit-2026-10-23.md。

### 拍板清单（按修复收益分组，建议按序开修复批）

**A. M5 UI 自动化整组（1 个文件群，5 P1 + 6 P2，建议整组一次修）**
P1：危险关键词大小写敏感；ui_type 换行绕确认（违背 schema 承诺）；敏感判定缺
AXSecureTextField/IsPassword 角色；Windows 坐标点击归一化 bug（落点失控）；后台窗口注入
打进前台任意应用。**→ 5 条 P1 已于 2026-10-23 逐条修掉（见上方「修复批次」）。**
P2：macOS 窗口 id 复用 TOCTOU；闸门快照与实际动作二次解析不一致；
无标题按钮绕确认（建议 fail-closed）；注入无全局互斥；开关变更无审计+flag 权限；授权模型
（应用白名单）是否要产品化——需要你拍方向。
**→ 前五条已于 2026-10-23 修掉（与 D 组合并成一个提交）；第六条不是缺陷、是要你拍方向，仍未动。**

**B. 凭据面（M12 三条 P2）**
① update_config_file 补 strip_verified_keys（keyring 故障窗口丢密钥，零丢失语义）；
② selfcheck_http 补 ensure_vendor_verified（侧道绕 vendor 门）；
③ 带厂商条目无 key 时禁止全局主 key 回退（防主力 key 发往任意 base_url + 探活 2xx 转正）。
**→ 三条已于 2026-10-23 逐条修掉（见上方「修复批次」）。**

**C. 数据一致性组**
M11 四条：合并搬进 with_write_conn+同事务；instr 子串越界改精确匹配；`__new__` 占位校验；
merge/rollback 补广播。M1 三条：软删补清 plan 字段（或订正注释+SPEC）；浮窗 due 套
isValidDateTimeLocal+后端补格式校验；GraphCanvas 铭牌 innerHTML 改 textContent。
M14 一行修：with_write_conn 改用 lock_db_write()（debug 构建 panic）。
**→ 八条已于 2026-10-23 修掉（见上方「修复批次」）。** M1 那条软删选了「订正口径」而非「清字段」：
调度两列早已退役、plan 是网格位置不是触发器、导出是全量备份——改代码会破坏可逆操作的语义。

**D. 跨平台契约（M15 两条 P2）**
macOS focused 实测前台（或 Option<bool>+声明）；Windows 读 UIA ValuePattern 或声明恒空。
**→ 两条已于 2026-10-23 修掉**（与 A 组余下五条 P2 合成一个提交——`ui/mod.rs` 改签名会让中间态编译不过）。

**E. P3 择优建议（其余 70+ 条见各批正文）**
僵尸 app_runs 行+启动清扫；孤儿通知两条（服务告警撤收、ask wfq 回收）；机器人开关关闭的
调度空转；Paused 技能超时；审计缺口三项（apps_run、配置变更/active 切换、consolidate 删除
内容）；bing/ocr 输入上限；M10 token 文件空残留自愈；archiveRule 配置未加载不应用；
taskEq 键序敏感。evolution/ 死目录随手清。
**→ 已修 7 条**（僵尸行+启动清扫 / token 文件空残留 / bing 大图 / OCR 上限 / Paused 超时 /
调度空转 / taskEq 键序）。**未修**：孤儿通知两条、审计缺口三项（apps_run 口径属 F 组第②条、
配置变更与 consolidate 删除内容待排）、archiveRule 配置未加载不应用、evolution/ 死目录清理。

**F. 待定性 11 处（需要你给口径，不急着修）**
① apps 是否纳管 py-enabled.flag；② apps_run 审计事件口径；③ 历史畸形 app id 是否存在
需清洗；④ open_url 工具文案补「跳转由浏览器接管」；⑤ opener capability 是否收窄 deny-open-url
（M13 结论：前端渲染面全文本节点+链接白名单，维持现状风险低）；⑥ HTTP SSE overlay 无
主窗写事件的轮询兜底确认；⑦ archiveRule 前后端恰 N 天边界逐行核对；⑧ tags_consolidate_scan
是否包 spawn_blocking；⑨ 受管服务 GBK 输出解码；⑩ Windows 长路径文案；⑪ DST 跳进算术
（国内不触达，备案）。

**G. 补审两批新增（2026-10-24，批 4 / 批 6；4 P1 + 7 P2 + 22 P3，均未修）**
按修复收益排序，建议开批顺序：

- **G1 · M2 批量执行触发词收紧（1 P1 + 相关 1 P3）**：CJK 触发词要求落在命令位，排除
  「什么时候/是否/别/先别/不用」等修饰；顺手把 `RouteAction::ExecuteTasks` 不可达臂收进
  `route_user_input_for` 一条路（否则将来两路会抢跑）。
- **G2 · M6 记忆导入组（2 P1 + 2 P2 + 3 P3）**：导入前声明容量策略（抬容量 or 满即 skipped）
  并让报告/注释/toast 如实显示淘汰数；解析与预嵌入移出 DB 写锁；合并时空 tags 不覆盖既有
  key/source/importance；画像段纳入注入预算；导入向量做维度校验；tags 含逗号按输入级违规计
  skipped 而非「存储故障」。
- **G3 · M8 MCP 发现与调用路径互斥（1 P1 + 1 P3 + 盲区 1 条）**：两条路径共用 per-server
  `call_lock`；spawn 在途加占位态；发现结束插 Idle 前复核槽位；顺带补跨服务器工具总量闸
  （量级 20MB/轮，先拍板再加）。
- **G4 · M9 受管服务权限开关（1 P2 + 1 P3 + 盲区 1 条）**：两道 flag 改 0600 + 收紧旧文件
  + 补审计事件（照 `ui::write_flag_file` 成例）；服务日志目录改 0700/0600 或落数据目录。
- **G5 · M7 技能会话档边界（2 P2 + 1 P3）**：`SkillInfo` 加 scope 字段并在清单层按会话过滤；
  `start_skill` 加会话归属校验；`install_skill` 确认窗补 intents + mode（照 `create_skill` 口径）；
  `skill_terminate_all` 的审计挪到锁外。
- **G6 · M2 其余 P2/P3（1 P2 + 5 P3）**：图片附件读前判大小；终稿 Y/N 出口闸绑定「自检仍悬着」
  而非「本提问问过自检」；SSE 两条 Err 出口补 `skill_finish`；task_refs 去重上提统一收口；
  非 2xx 响应体与 SSE 单行长度上限；汇报轮跳过 L0 规范化。
- **G7 · 意图准入探针（P3，建议与技能准入核一起加固）**：FILLERS 扩到 20+，探针改多字符×多长度
  组合，`probe == None` 不得直接跳过第三道闸。

**H. 补审两批新增的待定性 4 处（需要你给口径）**
⑫ 记忆导入「只增不删」是承诺还是实现漂移——若产品本意是「导入即收敛」，M6 P1-1 可降为 P2
（只改报告与文案）；若承诺成立，则要改实现（导入不淘汰，容量满计 skipped）。
⑬ MCP stdio 子进程是否该继承宿主全量环境变量（用户 shell 密钥会进第三方服务器）——服务侧
schema 已明写「继承」，MCP 侧无等价声明，是补声明还是补白名单。
⑭ `mcp_server_save` 半成功中间态（keyring 已写、配置未写）要不要回滚机密。
⑮ 跨服务器 MCP 工具总量闸加不加（加则牺牲外部工具可见性，不加则最坏 20MB/轮请求体）。
