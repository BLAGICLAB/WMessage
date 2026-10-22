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

**P0 0 条 / P1 5 条 / P2 20 条 / P3 73 条 / 待定性（NEEDS-VERIFICATION）11 处**；
P1 全部集中在 M5（桌面 UI 自动化），P2 分布：M5×6、M12×3、M11×4、M1×3、M15×2、M4×1、M14×1。

验证性结论：2026-10-21 的 23 项修复 **23/23 在位零回退**；2026-10-19 图谱轮修复抽查在位；
架构文档 §6.4 对拍 157=157；基线全绿（nextest 1981 / vitest 985 / tests-audit 29 / clippy 干净 /
machete 干净）。整体判断：**没有 P0**——安全边界（本地 API、keyring、vendor 门、MCP SSRF 闸、
压缩炸弹闸、脚本路径收窄）经逐条攻击性验证全部成立；本轮主病灶是**闸门判定材料可绕**
（UI 自动化）与**凭据/数据一致性边角**（keyring 故障窗口、合并无事务），多为上轮审计
「七链」之外的模块的首轮深审收益。

修复推进到 2026-10-23 收口：P1 五条 + P2 二十条**全部落地**（M5 的「授权模型」一条不是缺陷、
是要产品侧拍方向，另计），P3 择优七条落地。修完的基线：**Rust 单测 1867 通过 / 前端 1011 通过
/ tests-audit 29 通过 / clippy 干净 / 全量门禁全绿**。剩 F 组 11 处待你给口径。

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
