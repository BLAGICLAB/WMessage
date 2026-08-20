# 机器人工具升级计划 + 验收清单（2026-08-19）

> 背景：WMessage bot 的非任务卡工具弱于 Kimi CLI——搜索是 Bing/百度抓取+字符串解析（无排序）、
> fetch_url 无正文提取、无本地文件工具（安全红线未开）、模型 MiniMax-M3 工具能力弱。
> 用法同 ARCH-REFACTOR-PLAN：一次一阶段，完成一项勾一项，每阶段结束跑三件套
> （`cargo test` / `npx vitest run` / `npx tsc --noEmit`）全绿再进下一阶段。

## Phase 1：本地文件工具（白名单限定，把「不开」改成「可控地开」）✅

目标：新增 read_text_file / grep_files / list_files 三个工具，只能在白名单目录内操作。

- [x] 1.1 `bot-config.json` 增加 `allowed_dirs: Vec<String>`（设置页可编辑）；默认白名单：`~/Desktop`、`~/Downloads`、`~/Documents` + 所有任务卡绑定文件夹
- [x] 1.2 新模块 `src-tauri/src/bot_fs.rs`：`resolve_allowed(path)`——canonicalize 后必须落在白名单目录前缀内，拒绝 `..` 逃逸/软链逃逸/白名单外路径；每次访问记审计
- [x] 1.3 `read_text_file(path, offset?, limit?)`：UTF-8 文本读取，单文件 ≤2000 行/≤100KB，超长截断并标注；二进制/图片拒绝（图片走既有视觉通道）
- [x] 1.4 `grep_files(pattern, dir, glob?, max=50)`：正则内容搜索，ripgrep 式输出 `path:line:内容`，结果截断 50 条（注：未单独加 10s 超时，靠深度≤5/条目上限/单文件 2MB 上限兜底，同步遍历更快）
- [x] 1.5 `list_files(dir, pattern?, max=200)`：glob 匹配列文件，不递归超 5 层
- [x] 1.6 TOOLS schema + execute_tool 分发注册三工具；MUTATING_TOOLS 不加（只读）；tool_guard 非原子清单同步纳入
- [x] 1.7 prompt 补规则 19 + 红线更新为「白名单外一律拒绝」
- [x] 1.8 单测：glob 匹配 / ~ 展开 / 前缀相似目录不误判（/tmp/ab vs /tmp/abc）；resolve_allowed 全链路依赖 AppHandle+DB，靠这三项纯函数覆盖核心逻辑
- [x] 1.9 三件套全绿 ✅（cargo 377 / vitest / tsc）+ 手动冒烟 ✅（list_files/grep_files 白名单内正常；白名单外模型擅自换目录 → prompt 规则 19 已堵；extract_document 闸门兼容白名单目录）

## Phase 2：搜索/网页工具升级 ✅

- [x] 2.1 `fetch_url` 加正文提取：去 script/style/nav/footer 等噪声块 → article/main 标签 → 语义 class/id 最大 div → 兜底全文；结果 <100 字自动退回整页转换；html2text 保结构，30KB 截断保留
- [x] 2.2 `web_search` 结果结构化：摘要清理（空白压缩+结尾省略号去除）、同域名最多 2 条（百度跳转链接豁免）、来源标注 [Bing]/[百度]/[Tavily]
- [x] 2.3 可选接 Tavily 搜索 API（`bot-config.json` 加 `tavilyKey`，有 key 走 API、失败回退抓取并记审计 `web_search.tavily_fallback`）——设置页已加输入框；生效需老板申请 key
- [x] 2.4 单测：正文提取（nav/footer 剔除 + 语义 div 兜底）、嵌套同名标签移除、摘要清理、域名提取（5 个新增）
- [x] 2.5 三件套全绿 ✅（cargo 382 / vitest 100 / tsc）+ 手动冒烟（已验收）：让 bot 搜「今天的新闻」并总结一个链接正文

## Phase 3：模型可切换 ✅

- [x] 3.1 核对设置页是否已能编辑 base_url/model；不能则补上 —— 已有（SettingsPage 的 Base URL / 模型输入框 + bot_set_config 链路完整），无需补
- [x] 3.2 验证 OpenAI 兼容 payload 对 Kimi（`https://api.moonshot.cn/v1`）可用：tools 格式、SSE 流式、tool_calls 增量解析 —— 代码核对通过：payload（model/messages/tools/stream + Bearer）、SSE 解析（`data:` / `[DONE]` / `delta.content` / `delta.tool_calls` index/id/name/arguments 增量拼接）、tool 回填（assistant.tool_calls + role:tool + tool_call_id）全是标准 OpenAI 规范，Kimi 官方兼容；实战验证并入 3.4
- [x] 3.3 设置页加提供商预设（MiniMax / Kimi K3 / DeepSeek V4 Flash / DeepSeek V4 Pro），点击自动填 base_url+推荐模型，命中预设高亮（baseUrl+model 双匹配——DeepSeek 两档同 baseUrl）；聊天窗口头部加 🧠 模型快速切换菜单（预设 + ✏️ 自定义自由填 base_url/model），设置页保存经 bot-config-changed 事件同步刷新
- [x] 3.4 手动冒烟（已验收）：切 Kimi K3 后重跑幻觉场景（「删除某任务卡」），工具调用正常；踩坑：401 是 keychain 里旧 MiniMax key 没换，换 key 后正常
- [x] 3.5 已知坑记录进 DEVLOG：不同模型对 prompt 规则敏感度不同，幻觉守卫是模型无关兜底；切换提供商必须同步换 keychain 里的 API Key

## Phase 4：补几个低成本高价值工具 ✅

- [x] 4.1 `get_current_time`：返回当前日期时间+星期（模型做「今天/明天」类判断不再瞎猜）——prompt 规则 20 强制先调
- [x] 4.2 `remember_fact(key, value)` / `recall_facts()`：简易长期记忆（SQLite 新表 `bot_facts`，key 主键 upsert、空 value 删除、200 条上限）——老板确认做；模型主动式（prompt 规则 21 引导，不自动注入全量，防白烧 token）；remember_fact 计入 MUTATING_TOOLS（幻觉守卫覆盖「已记住」话术）
- [x] 4.3 三件套全绿 ✅（cargo 387 / vitest 103 / tsc）+ 手动冒烟 ✅（跨会话回忆验证通过）

## 总验收

- [x] V1. 三件套全绿（cargo 387 / vitest 103 / tsc 零错）
- [x] V2. bot 能在白名单内读/搜文件，白名单外访问被拒且有审计（Phase 1 冒烟已验）
- [ ] V3. 搜索+网页总结体感对比（同一问题问 WMessage bot 和 Kimi CLI）
- [x] V4. 切 Kimi K3 跑一遍任务卡操作全链路（Phase 3.4 冒烟已验：删除任务卡真调 delete_task）
- [x] V5. DEVLOG.md 补记录
