# DeepSeek 搜索引擎接入方案（web_search 第三路由 + fetch_url 抓取开关）

> 2026-11-01 落地。上游调研：中文 AI 搜索 API 对比（DeepSeek/博查/智谱/Kimi/百度千帆/秘塔），
> 结论 DeepSeek 综合最优（价格全场最低、返回结构为 agent 设计、复用现有 DeepSeek vendor key）。

## 一、背景

- 现有 web_search 三路线（Bing+百度抓取 / Tavily / Brave）中文质量与 DeepSeek 自研搜索引擎差距明显：
  抓取路线无 SLA（百度触发验证页即空手而归）、Tavily basic 档浅、Brave 中文弱。
- DeepSeek 已把搜索做成公开 API（2026-10 中旬上线，2026-11-01 起计费）。
- 用户拍板 A 方案：接 DeepSeek Search API 作第三引擎（对齐 `resolve_search_route` 互斥模式）+
  Fetch API 一起接。

## 二、调研结论（单价，2026-11-01 核实）

| 服务 | 单价 | 备注 |
|---|---|---|
| **DeepSeek Search/Fetch** | **0.005 元/次**（失败/空结果/超时不计费） | 自研索引、日数亿次；结果带 rerank_score/authority/date |
| 博查 Web Search | 0.036 元/次 | 中立供应商，实测口碑最稳，备选 |
| 智谱 search_std/pro | 0.01 / 0.03 元/次 | 若已有智谱 key 顺手 |
| Kimi search basic/pro | ~0.01 / 0.015 元/次 | 绑 Moonshot 生态 |
| 百度千帆 AI 搜索 | 刊例 ~0.01 元/次（促销 4.9 元/千次） | 中文索引强，云账号门槛高 |
| 秘塔 | 0.03 元/次 | 自建索引、多模态 |

## 三、API 事实（官方文档核实）

- Search：`POST https://api.deepseek.com/v1/tools/search`，`Authorization: Bearer <key>`；
  body：`query`（1–128 Unicode 字符，必填）、`count`（1–50 默认 10，我们钳 1–10）；
  查询运算符原样转发：`site:` / `-site:` / `"…"` / `lang:` / `after:` / `before:`；
  响应 `results[]`：`rerank_score / title / url / snippet(100–200字) / summary(600–800字) /
  site_name / date(RFC3339) / authority`。100 QPS。
- Fetch：`POST https://api.deepseek.com/v1/tools/fetch`，body `{url}`（≤8192 字符、http/https、
  不含 userinfo）；响应 `{title, content(Markdown)}`；25 QPS（账户级，另有单域名限制）。
- 错误码：401 key 无效 / 402 余额不足 / 403 安全拒 / 404 无内容 / 429 QPS。
- 计费：按成功请求计费（HTTP 200 且有结果），count 不影响单价。

## 四、拍板记录

1. **A 方案**（用户拍板）：DeepSeek Search API 作第三引擎，对齐现有三选一互斥模式；设置页加开关。
2. **Fetch 接法**（计划阶段问询未答，按推荐执行兜底腿；**实施中被用户改向**）：
   - 初案：fetch_text 兜底腿（直连失败 → DeepSeek → Jina，随「DeepSeek 搜索」开关自动启用）。
   - **终案（用户改向）：取消兜底腿，改设置页 MCP 模块手动开关「DeepSeek 抓取」**——
     严格手动（Some(true)+key 才启用，不做 key 自动态）；开启后 fetch_url **优先**走 DeepSeek
     Fetch，失败回退原直连+Jina 链路（回退只发生在 DeepSeek 失败时，与被否的「直连失败自动
     启用 DeepSeek」方向相反）；默认关，原链路零变化。理由：核心工具抓取走第三方是口径变化，
     必须显式开启。
3. **搜索输出用 snippet+date，不用 summary**：summary 600–800 字/条必撞 SEARCH_OUTPUT_CAP_BYTES(6000B)
   输出闸（截断吃掉后面的结果）；质量增益主要来自引擎排序与覆盖，读正文由 fetch_url 承担。
4. **独立 KeySlot::Deepseek**（不复用 vendor 凭据机制）：对齐 Tavily/Brave 模式；设置页文案注明
   与 DeepSeek 大模型 API 同一把平台 key。
5. **不接** `localization`（IP 隐私）、`ori_question`、`need_content`（体积）。

## 五、实现落点

- 后端：`bot/config/types.rs`（KeySlot::Deepseek + deepseek_enabled/deepseek_fetch_enabled）、
  `io.rs`（迁移臂）、`commands.rs`（bot_set_config/bot_get_config）、
  `bot_web/search.rs`（SearchRoute::Deepseek/MissingDeepseekKey/Conflict(引擎名列表) +
  search_deepseek + 纯函数 deepseek_query_and_time/deepseek_time_suffix/parse/format）、
  `bot_web/fetch.rs`（deepseek_fetch_page + deepseek_fetch_mode 决策 + FetchedText.deepseek）、
  `bot/tools/doc.rs`（tool_fetch_url 读开关下传 + 审计）、`bot/registry/schemas.rs`（schema 文案）。
- 前端：`McpSection.tsx`（DeepSeek 搜索块 + 抓取块 + 多开冲突泛化）+ 三 hook + types/botConfig。
- 审计事件：`web.deepseek_fetch_used` / `web.deepseek_fetch_failed`（audit_event!，已收录
  event-names.md）；`web_search.deepseek_failed`（audit_log 字符串，与 tavily/brave_failed 同类不收录）。
- 门禁：SOP 常量表与工具总数 66 **不动**（无新 constants.rs 常量、无新工具，端点内联 search.rs
  同 Tavily/Brave）；tools_baseline.json 已重生成（web_search schema 文案）。

## 六、测试

- `bot_web/tests.rs`：路由 5 新例（DeepSeek 单开/缺 key/显式关/三开与双开 Conflict 列名）+
  query 映射 + time 后缀（含月末钳制）+ parse 2 例 + format 日期渲染 + http 错误映射 +
  deepseek_fetch_mode 决策矩阵。
- `SettingsPage.test.tsx`：DeepSeek 搜索开关持久化 / 缺 key 提示 / 抓取开关警告+点关持久化 /
  三引擎多开警告。
- 验收卡 ACC-P-12（SPEC 6-117）：真 key 中文搜索、时间/站点过滤、多开报错、计费对账、
  抓取开关与失败回退。

## 七、偏离登记

1. Fetch 接法中途改向（兜底腿 → 手动开关），见 §四.2——计划文本与终案不一致处以此为准。
2. MCP 卡重构成 EngineToggle 子组件（三引擎开关 UI 去重）——超出「照搬 Tavily 块」的原计划，
   属等价改写，存量 Tavily 测试全绿佐证 DOM 结构未变。
3. 冲突报错从 `Conflict` 无载荷改为 `Conflict(Vec<&'static str>)` 携带引擎名——报错文案对
   Tavily+Brave 双开场景与旧文案逐字一致，存量测试仅改断言目标不改语义。

## 八、审计修复（2026-11-02，2 P2 + 4 P3，拍板两项）

审计结论：无 P0/P1，核心防线（密钥三层 / 迁移五臂 / SSRF 先闸后发 / 路由零语义回归）确认成立。
逐项处置：

| 发现 | 处置 |
|---|---|
| P2-1 三腿全灭路径丢 DeepSeek 实因 + 审计不落 | **拍板：只并入错误文案**——终错带「DeepSeek 抓取（实因）也没成」；全灭路径不落审计事件（stderr + 文案是仅有的留痕），SPEC 6-117 措辞同步收紧 |
| P2-2 双保险回归测试缺 deepseek_key + 三处「三个」陈旧注释 | 测试补第四槽断言；三处注释改为「各明文槽位」 |
| P3-1 keyring 故障细节被丢 | eprintln 带错误体 |
| P3-2 128 字符守卫无测试 | 抽纯函数 `deepseek_query_limit_error` + 3 组边界测试（129 超限 / 128 放行 / 组装后越线） |
| P3-3 前后端冲突分隔符漂移（、 vs 与） | 前端统一「与」，与后端 Conflict 文案同源 |
| P3-4 空结果错误带误导建议 | **拍板：接受同款**——与 Tavily/Brave 现状一致，不单改 |
| 观察：只为抓取配 key 会自动点亮 DeepSeek 搜索 | 接受（UI 可见非静默，多开互斥仍生效）；已在本节留痕 |
| 观察：404 文案 fetch 口径 / schema「每条带日期」轻微过称 | 接受（共享 helper 与 date 可选的已知代价） |
