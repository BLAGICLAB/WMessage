# ACC-P · 数据、集成与平台（L2 实机卡）

> 覆盖 `SPEC.md`「6. 数据、集成与平台」9 条功能（6-106 ~ 6-114），对应矩阵 `docs/acceptance/matrix.csv` 的 D6 域。
> 判据写法与证据规范见 `docs/ACCEPTANCE-PLAN-2026-10-30.md`；审计事件名取自 `docs/acceptance/event-names.md` 白名单。
> 数据目录（macOS）：`~/Library/Application Support/com.renshi.wmessage/`（恒定落点，不随 exe 走，机制见 ACC-P-03）。下文的 `<data_dir>` 指该目录。
> 日志检索统一写法：`grep -F "<事件名>" "<data_dir>/bot.log" | tail -20`。

### ACC-P-01 · SQLite：WAL + busy_timeout 2s + 单写者行级增量

- **SPEC**：6-106
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：M57、M69、M70、M71（`docs/realmachine-mutation-test-2026-10-24.md`）、冒烟 4/4b（`docs/realmachine-smoke-report-2026-10-25.md`）
- **前置**：应用已启动并跑过至少一轮对话；数据目录里已有 `<data_dir>/wmessage.db`
- **步骤**：
  1. 在任务页改一张卡的标题（双击网格块开编辑浮窗，改完保存）。
  2. 切到首页看 KPI、切到图谱看节点、切到聊天右侧栏「任务」工作面看任务池标题。
  3. 应用运行中在数据目录里看有没有 WAL 边车文件。
  4. 用 `sqlite3` 依次查 journal_mode、busy_timeout、integrity_check、表数。
  5. 查 `<data_dir>/data.json` 旧数据迁移链：在数据目录里看有没有 `data.json.migrated` 留存文件。
- **判据**：
  1. 【可核对】`sqlite3 "<data_dir>/wmessage.db" "PRAGMA journal_mode;"` 返回 `wal`。
  2. 【可核对】`sqlite3 "<data_dir>/wmessage.db" "PRAGMA busy_timeout;"` 返回 `2000`。
  3. 【可核对】`sqlite3 "<data_dir>/wmessage.db" "PRAGMA integrity_check;"` 返回 `ok`。
  4. 【可核对】`sqlite3 "<data_dir>/wmessage.db" "select name from sqlite_master where type='table';"` 命中 `tasks`、`bot_sessions`、`bot_messages`、`mem_items`、`exec_traces` 五张表，且行数 ≥ 21（当前源码建表口径）。
  5. 【可观测】应用运行中数据目录里存在 WAL 边车 `<data_dir>/wmessage.db-wal` 与 `<data_dir>/wmessage.db-shm`。
  6. 【可观测】步骤 1 改标题后 1 秒内，首页、图谱、聊天右侧栏任务池三处都显示新标题——增量落盘并广播，不是整页重载。
  7. 【可核对】`grep -n "busy_timeout" -A 3 src-tauri/src/db/mod.rs` 命中 `Duration::from_secs(2)`——busy_timeout 有意保留 2s，不调 5000ms（进程内写者已由写锁串行化）。
  8. 【可核对】`grep -n "journal_mode=WAL" src-tauri/src/db/migrations.rs` 命中；`grep -n "DB_WRITE_LOCK" src-tauri/src/db/mod.rs` 命中——单写者。
  9. 【可核对】`grep -n "data.json.migrated" src-tauri/src/db/migrations.rs` 命中——旧 `<data_dir>/data.json` 迁进库后原文件改名留存（localStorage 迁移已删）。
- **证据**：截图（改标题后三处视图）+ `sqlite3` 四条查询的终端输出 + 数据目录文件清单
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（本卡无专属审计事件，改查 `grep -n "busy_timeout" src-tauri/src/db/mod.rs`：值不是 2s = 卡片过期）
- **缺口补验**：WAL 与 busy_timeout、单写者行级增量未专卡

### ACC-P-02 · 本地 HTTP API：端点面 + token 鉴权 + SSE

- **SPEC**：6-107
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：M85（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：应用已启动；设置页 → 「机器人」分区能打开本地 HTTP API 卡
- **步骤**：
  1. 设置页 → 「机器人」分区 → 本地 HTTP API 卡：打开开关，记下端口与令牌。
  2. `curl` 依次打 `/api/health`（不带 token）、`/api/tasks`（不带 token）、`/api/tasks`（带错 token）、`/api/tasks`（带对 token）。
  3. 另开一个终端 `curl -N` 连 `/api/events`（带 token），保持连接；再在应用里新建一张任务卡，看 SSE 流里有没有事件。
  4. 点「重新生成」换令牌，用旧令牌再打一次 `/api/tasks`，用新令牌再打一次。
  5. 停掉开关，确认端口不再监听。
- **判据**：
  1. 【可观测】本地 HTTP API 卡上有开关、端口号、访问令牌（Bearer Token）输入框、「复制」「重新生成」两个按钮，以及接口说明列表（列出 `GET /api/health` 免鉴权、`GET /api/tasks`、`POST /api/tasks`、`GET /api/tasks/{id}`、`GET /api/events`）。
  2. 【可核对】`curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:<端口>/api/health"` 返回 `200`，响应体含 `"ok":true` 与 `"service":"wmessage-api"`——健康检查是唯一免鉴权端点。
  3. 【可核对】`/api/tasks` 不带 token 返回 `401`、带错 token 返回 `401`（响应体含 `"error":"unauthorized"`）、带对 token 返回 `200`。
  4. 【可核对】`curl -N "http://127.0.0.1:<端口>/api/events" -H "Authorization: Bearer <令牌>"` 建立的连接响应头是 `Content-Type: text/event-stream`；步骤 3 里新建任务卡后，SSE 流收到该任务的事件。
  5. 【可核对】步骤 4：换令牌后旧 token 立刻失效（`/api/tasks` 返回 401），新 token 返回 200。
  6. 【可核对】步骤 5：关掉开关后该端口不再 LISTEN（`lsof -nP -iTCP:<端口> -sTCP:LISTEN` 无输出）。
  7. 【可核对】`grep -n "api_start\|api_stop\|api_status\|api_rotate_token" src-tauri/src/lib.rs` 四个命令齐全；`grep -n "verify_bearer" src-tauri/src/api_auth.rs` 命中鉴权口径。
  8. 【可核对】令牌落盘在 `<data_dir>/runtime/flags/api-token.txt`（权限 0600），开关标志在 `<data_dir>/runtime/flags/api-enabled.flag`——不落明文配置。
  9. 【可审计】服务异常时 `<data_dir>/bot.log` 中出现 `api.handler_panic`（处理器 panic）或 `api.recv_error`（连接错误）（检索命令：`grep -F "api.handler_panic" "<data_dir>/bot.log" | tail -20`）。
- **证据**：截图（本地 HTTP API 卡）+ 每条 `curl` 的状态码与响应体 + `<data_dir>/bot.log` 片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（`api.handler_panic` / `api.recv_error` grep 不到 = 卡片过期）
- **缺口补验**：只验了端口采集金丝雀：端点面、token 鉴权、多用户信封、SSE 未专卡

### ACC-P-03 · 数据目录落点：恒 app_data，程序目录与数据目录分离

- **SPEC**：6-108
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：W01（`docs/realmachine-mutation-test-2026-10-23.md`）、W10、S31（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：已有一个能跑的构建产物（exe / .app）
- **步骤**：
  1. 把应用放进任意可写目录（或走安装版）启动，跑一轮对话。
  2. 看数据落点：库 / 日志 / `AI_Gen_Files` 必须在 app_data 目录，exe 目录旁不出现。
  3. 在 exe 目录预放 `wmessage.db` 与 `AI_Gen_Files/` 再启动——历史数据痕迹不再锚定数据目录，数据仍落 app_data。
  4. （Windows 安装版）卸载勾选「删除应用数据」→ `%APPDATA%\com.renshi.wmessage` 消失；不勾选 → 保留。
- **判据**：
  1. 【可观测】步骤 2：`<data_dir>/wmessage.db`、`<data_dir>/bot.log` 与 `AI_Gen_Files/` 全部在 app_data 目录；exe 目录旁无新增数据文件。
  2. 【可观测】步骤 3：exe 旁预放的数据痕迹不被锚定，数据仍落 app_data 目录。
  3. 【可核对】`grep -n "fn probe_dir_cached_in\|fn probe_dir\b" src-tauri/src/paths.rs` 命中且 `probe_dir` 函数体内无写探针、无数据痕迹锚定——数据目录判定只有一处，首次探测结果钉死、后续条件变化也不翻转。
  4. 【可核对】`grep -n "RUNTIME_SUBDIR" src-tauri/src/paths.rs` 命中且值为 runtime——runtime/ 子目录（API token 等）仍在数据目录内。
- **证据**：截图（app_data 与 exe 目录文件清单）+ 启动日志片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——`grep -n "fn probe_dir" src-tauri/src/paths.rs`：判定逻辑不在 = 卡片过期；逻辑在但落点不对 = 应用侧回归
- **缺口补验**：—

### ACC-P-04 · 交付给用户的产物不带「来自互联网」标记

- **SPEC**：6-109
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：W03（`docs/realmachine-mutation-test-2026-10-23.md`）
- **前置**：应用已启动；机器人总开关已打开；能生成文档产物（Word / Excel / PPT / PDF 任一）
- **步骤**：
  1. 让机器人生成一份文档产物（例：「生成一份 2026 年第三季度复盘 Word 文档」），记下产物绝对路径。
  2. 在产物目录里对该文件查备用数据流。
  3. 另造一个**用户自己的、带「来自互联网」标记的源文件**（Windows：从浏览器下载一个文件；macOS 无此标记则跳过本步），把它作为模板或被引用文件喂给机器人。
  4. 处理完后查这个源文件本身。
  5. 另跑一次 Word 修订模式产物（`word_revisions` 出口）与其余三个出口，确认五个出口一致。
  6. **复制 / 搬运路径**（2026-11-02 新增覆盖）：让机器人把那份带标记的文档**复制**到产物目录（或用桌面清理把它**归档**到另一目录），对副本查备用数据流。
  7. 对同一个源文件跑一次**搬运**（改名到别的目录），查落点上的备用数据流；再查源路径（应已不存在——搬运是改名，不是复制）。
  8. 另造一个带标记的**可执行文件**，走同一条复制路径，对副本查备用数据流（可执行文件不收口，标记应当仍在）。
- **判据**：
  1. 【可观测】步骤 2：产物文件上**查不到「来自互联网」标记**——Windows 上 `Get-Item -Path "<产物>" -Stream *` 的列表里没有 `Zone.Identifier` 这一条；macOS 上不存在该备用流。
  2. 【可核对】`grep -n "ZONE_STREAM" src-tauri/src/platform/zone_id.rs` 命中且值为 `Zone.Identifier`——只删这一条流，主数据流与其余流不动。
  3. 【可核对】`grep -rn "strip_zone_identifier" src-tauri/src/py/` 命中五个文档产物出口（word / word_revisions / excel / pdf / ppt）加 Word 模板上传与 run_python 产物搬运的调用点——六个出口走同一个收口。
  4. 【可观测】步骤 3/4：**用户自己的原文件不动**——源文件的「来自互联网」标记在处理后仍在（标记是用户的安全策略，app 不擅自改）。
  5. 【可观测】剥不掉时只记审计不阻断：命令仍然成功、文件仍然交付，模型侧收到的是「文件本身是好的」这一事实，不是失败。
  6. 【可核对】`grep -n "fn strip_zone_identifier" -A 8 src-tauri/src/platform/zone_id.rs` 命中，且非 Windows 分支是空实现（macOS 上不做任何事，也不报错）。
  7. 【可观测】步骤 6/7：**副本与搬运落点上查不到 `Zone.Identifier`**（文档 / 图片类才收口）——否则 Office 打开它走受保护视图、写文档缓存，杀软按风险程序连坐拦掉。
  8. 【可观测】步骤 8：可执行文件的副本**仍带**标记——那里的标记是实质安全信号（SmartScreen / 杀软 / 解压继承），app 不擅自替用户改安全策略。
  9. 【可核对】`grep -rn "zone_id::copy_file\|zone_id::strip_after_move" src-tauri/src/` 命中 manage_file、桌面清理归档、技能导入三处收口调用点。
- **证据**：截图（产物目录 + 备用数据流列表）+ 终端输出 + 源文件处理前后的对比
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（本卡无专属审计事件，改查 `grep -rn "strip_zone_identifier" src-tauri/src/py/`：调用点不在 = 卡片过期；调用点在但产物仍带标记 = 应用侧回归）
- **缺口补验**：—

### ACC-P-05 · 本机运行环境监测：18 Python / 3 Node / 16 运行时

- **SPEC**：6-110
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：S07、S31、S32（`docs/realmachine-regression-test.md`）、M31、M34（`docs/realmachine-mutation-test-2026-10-23.md`）
- **前置**：应用已启动；设置页可进「机器人」分区
- **步骤**：
  1. 设置页 → 「机器人」分区 → 「本机运行环境」卡，点检测（或刷新）。
  2. 展开三组折叠区，数每一组的条目数。
  3. 在 Python 组里找 openpyxl / python-docx / python-pptx / pypdf / reportlab 五项，看它们是否被单独标为硬依赖。
  4. 故意制造一个缺项（例如用干净虚拟环境跑一次），看缺项行是否给出安装命令。
  5. 看 Node 组的条目；看运行时组里 node / npm 归在哪一组。
- **判据**：
  1. 【可观测】设置页「机器人」分区有一张**独立卡**「本机运行环境」（不塞进「机器人设置」开关卡里），卡内三组折叠展示：Python 包 / Node 全局包 / 运行时与启动器。
  2. 【可观测】Python 组恰好 18 项，Node 组恰好 3 项，运行时组恰好 16 项（三组条目数对得上）。
  3. 【可观测】五个文档硬依赖（openpyxl / python-docx / python-pptx / pypdf / reportlab）在 Python 组里标为硬依赖；缺任意一个时**单独标出**，与十三项办公增强项区分。
  4. 【可观测】Node 组恰好三项：pptxgenjs、@mermaid-js/mermaid-cli、@agent-canvas/cli。
  5. 【可观测】node 与 npm 出现在**运行时组**（MCP 启动器靠它们判断能不能起来），不在 Node 全局包组。
  6. 【可观测】缺项行给出安装命令：Python 组一条 `python3 -m pip install …`（Windows 上是 `python -m pip install …`）；Node 组一条 `npm i -g …`；运行时组逐项给（每个工具装法不同，一条命令装不齐）。
  7. 【可观测】探针输出覆盖不全时显示「探测失败」，不是把没判的报成缺；探针线程 panic 时显示「探测异常」，不是空组。
  8. 【可核对】`grep -n "PY_PACKAGES" -A 40 src-tauri/src/env_probe.rs` 命中且列出 18 项；`grep -n "NODE_PACKAGES" -A 6 src-tauri/src/env_probe.rs` 命中且列出 3 项；`grep -n "LAUNCHERS" -A 100 src-tauri/src/env_probe.rs` 命中且列出 16 项——三组清单在后端一处，平台知识不散在前端。
  9. 【可核对】`grep -n "find_spec" src-tauri/src/env_probe.rs` 命中——探测用 `importlib.util.find_spec` 不用 import（import 18 个包会撞穿 3s 探针超时，被超时杀掉会把「装了」全报成「缺」）。
- **证据**：截图（「本机运行环境」卡三组展开态）+ 检测输出
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（本卡无专属审计事件，改查 `grep -n "PY_PACKAGES" src-tauri/src/env_probe.rs`：清单不在 = 卡片过期；清单在但条数与卡上写的对不上 = 卡片过期）
- **缺口补验**：18 Python 包 / 3 Node 包 / 16 运行时三组的缺项与安装命令未全验

### ACC-P-06 · 桌面清理：CSV 规则表 + {year} 占位 + move/delete 两动作

- **SPEC**：6-111
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：—（无：全库无实机卡）
- **前置**：应用已启动；设置页可进「文件整理」分区；准备两个测试文件（文件名分别含「工资」和「临时」），放在一个专门的测试目录里
- **步骤**：
  1. 设置页 → 「文件整理」分区，点「下载表格模版」，用 Excel / WPS 打开看表头与示例行。
  2. 在模版里加两行：一行动作「移动归档」、归档目录「工资/{year}」、关键字「工资」；一行动作「删除文件」、关键字「临时」、归档目录留空。导入规则表。
  3. 点「立即执行绑定整理」；再到测试目录里看两个文件的去向。
  4. 点「扫描整理」（对各整理来源中的**未绑定**文件），再看一次。
  5. 给一张任务卡绑定一个文件名含「工资」的文件，再执行一次整理，看这个文件与卡片绑定。
  6. 打开整理记录弹窗读日志。
- **判据**：
  1. 【可观测】「文件整理」分区有规则表卡：「下载表格模版」「导入规则表」两个按钮，规则用表格管理，规则顺序即优先级（靠前的行先匹配）。
  2. 【可核对】下载到的模版是 CSV，表头恰为四列：启用 / 文件名关键字 / 动作 / 归档目录；示例行含关键字「工资，报销」、动作「移动归档」、归档目录「工资/{year}」。
  3. 【可核对】`grep -n "POLL_INTERVAL_SECS" src-tauri/src/migration/run.rs` 命中且值为 600——10 分钟后台轮询。
  4. 【可观测】步骤 3：文件名含「工资」的文件被移到归档目录，且目录名里的 `{year}` 已被替换成当前年份（目录名里不出现字面 `{year}`）。
  5. 【可观测】步骤 3：文件名含「临时」的文件被删除；同一目录里不命中任何关键字的文件原地不动。
  6. 【可观测】步骤 4：「扫描整理」对来源目录里的**未绑定**文件执行同一套规则，被任务卡绑定的文件跳过。
  7. 【可观测】步骤 5：**看板/回收站附件绝不触碰**——绑定了「工资」文件的卡片，执行整理后该文件仍在原位，卡片上的文件绑定不丢。
  8. 【可核对】`grep -n "fn parse_rules_csv" -A 20 src-tauri/src/migration/rules.rs` 命中——CSV 四列表头解析与动作归一（移动归档 / 移动 → move，删除文件 / 删除 → delete）。
  9. 【可核对】`grep -n "fn validate_rules" -A 20 src-tauri/src/migration/rules.rs` 命中——move 必须有归档目录、非法动作被拒并给出第几条规则出错。
  10. 【可核对】`grep -rn "LOG_FILE" src-tauri/src/migration/rules.rs` 命中——执行记录落 `<data_dir>/migration.log`，与设置页「日志」分类的「文件迁移」来源同源。
- **证据**：截图（模版表格内容 + 规则表 + 整理前后文件去向 + 整理记录弹窗）+ 终端文件清单
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（本卡无专属审计事件，改查 `grep -n "POLL_INTERVAL_SECS" src-tauri/src/migration/run.rs`：值不是 600 = 卡片过期）
- **缺口补验**：无实机卡：CSV 规则表、{year} 占位、move/delete 两动作、10 分钟轮询

### ACC-P-07 · 日志体系：四个落盘日志 + 5MB 轮转两代

- **SPEC**：6-112
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：M87（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：应用已启动并跑过至少一轮对话；数据目录可读
- **步骤**：
  1. 在数据目录里列出四个落盘日志文件。
  2. 看 `<data_dir>/bot.log` 的行格式；跑一次带工具调用的对话，看新行长什么样。
  3. 制造一次 WARN / ERROR（例如访问一个不存在的路径），看 `<data_dir>/error.log` 里有没有对应行。
  4. 把 `<data_dir>/bot.log` 写到 5MB 以上，再触发一次写入，看轮转产物。
  5. 设置页 → 「日志」分类，切三段来源；点「复制全部日志」。
- **判据**：
  1. 【可观测】数据目录下有四个落盘日志：`<data_dir>/bot.log`（全应用结构化审计）、`<data_dir>/error.log`（WARN/ERROR 镜像账本）、`<data_dir>/api.log`（HTTP API 访问 + 变更）、`<data_dir>/migration.log`（迁移执行）。
  2. 【可核对】`<data_dir>/bot.log` 的行格式统一为 `[ts] LEVEL | event | k=v`；`grep -F "llm.request" "<data_dir>/bot.log" | tail -5` 每行都能拆出时间戳、级别、事件名与 kv 段。
  3. 【可核对】步骤 3：对应的那条 WARN/ERROR 行在 `<data_dir>/error.log` 里也存在（镜像账本，不被 Info 流水稀释）；同一行在 `<data_dir>/bot.log` 里也在。
  4. 【可核对】步骤 4：`<data_dir>/bot.log` 超过 5MB 后再写入 → 出现 `<data_dir>/bot.log.old` 与 `<data_dir>/bot.log.old.1`，更早的一代被丢弃（只保留两代）。
  5. 【可观测】设置页「日志」分类有来源分段切换（错误 / 机器人审计 / 文件迁移），每个来源读末条 200 行；「复制全部日志」把 error 信号在前、流水在后，单文件超 256KB 尾部截断。
  6. 【可核对】`grep -rn "LOG_ROTATE_BYTES" src-tauri/src/db/paths.rs` 命中且值为 5MB 档；`grep -n "ERROR_LOG_NAME" src-tauri/src/audit.rs` 命中且值为 error.log。
  7. 【可核对】`grep -rn "frontend_event_report" src-tauri/src/` 命中——前端 window error / unhandledrejection 与 CSP 违规经该命令上报（32 签名去重）。
- **证据**：截图（数据目录日志清单 + 设置页「日志」分类三段切换）+ `<data_dir>/bot.log` / `<data_dir>/error.log` 片段 + 轮转前后的目录清单
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（本卡以日志文件名与格式为准；`bot.log.old.1` / `error.log.old.1` 等轮转名在白名单里，grep 不到 = 卡片过期）
- **缺口补验**：—

### ACC-P-08 · 全局快捷键：唤起/隐藏 + 快速新建 + 切换主题

- **SPEC**：6-113
- **层级**：L2 实机
- **历史卡**：—（无：全库无实机卡）
- **前置**：应用已启动；主窗口可见；当前主题已知（浅色或深色）
- **步骤**：
  1. 主窗口可见时按 macOS `Cmd+Ctrl+W`，观察窗口去向；再按一次，观察是否被唤起。
  2. 换个应用在前台，按 macOS `Cmd+Ctrl+N`，观察 wmessage 是否被唤起并进入标题编辑。
  3. 按 macOS `Cmd+Ctrl+T`，看主题是否切换；重开应用看主题是否保持。
  4. 让另一个应用先占住 `Cmd+Ctrl+W`（macOS：系统设置里给别的应用绑同一个快捷键），再启动 wmessage。
- **判据**：
  1. 【可观测】主窗口可见时按 `Cmd+Ctrl+W`：窗口隐藏（切走，不是退出——托盘图标仍在、进程未退）。
  2. 【可观测】主窗口隐藏时按 `Cmd+Ctrl+W`：主窗口被唤起并置于最前。
  3. 【可观测】按 `Cmd+Ctrl+N`：主窗口被唤起，并直接进入新建任务的标题编辑态（任务编辑浮窗打开、焦点在标题输入框），不需要再点一次「新建任务」。
  4. 【可观测】按 `Cmd+Ctrl+T`：主题切换一次（浅色 ↔ 深色），且切换结果持久化——重开应用仍是切换后的态。
  5. 【可观测】步骤 4：快捷键被别的应用占用时，wmessage **照常启动**（不崩、不卡在启动页），只有日志里有注册失败告警。
  6. 【可核对】`grep -n "hotkey_mods" -A 5 src-tauri/src/lib.rs` 命中——macOS 用 Cmd+Ctrl，Windows/Linux 用 Ctrl+Alt。
  7. 【可核对】`grep -n "Code::KeyW\|Code::KeyN\|Code::KeyT" src-tauri/src/lib.rs` 三处齐全；注册失败只 `log::warn!` 不让启动失败。
- **证据**：录屏（三次按键的前后对照）+ 截图（唤起态 / 标题编辑态 / 主题切换态）
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（本卡无专属审计事件，改查 `grep -n "Code::KeyW\|Code::KeyN\|Code::KeyT" src-tauri/src/lib.rs`：键位不在 = 卡片过期；键位在但按了没反应 = 应用侧回归）
- **缺口补验**：无实机卡：三个全局快捷键未验

### ACC-P-09 · UI 体系：nm 组件类 + 深浅色三态 + a11y 基线

- **SPEC**：6-114
- **层级**：L2 实机
- **历史卡**：—（无：全库无实机卡）
- **前置**：应用已启动；系统处于默认字号
- **步骤**：
  1. 设置页 → 「基础设置」→ 主题选项，逐个选「浅色」「深色」「跟随系统」，看界面变化。
  2. 在首页、任务页、聊天页、设置页各截一张图，观察卡片/按钮/输入框的立体处理。
  3. 把系统字号调到特大，看左导航项文字。
  4. 用 Tab 键在主界面走一圈，看焦点环；打开聊天页看当前会话条目；打开一个确认弹窗看其可读属性。
  5. 切到深色再看一遍第 2 步。
- **判据**：
  1. 【可观测】主题是**三态**：浅色 / 深色 / 跟随系统；选「跟随系统」时随系统外观自动切换。
  2. 【可观测】主界面控件走新拟态 nm 体系：卡片、按钮、输入框带同色高光 + 柔和阴影（不是纯扁平灰底）；深浅两套主题下都成立。
  3. 【可观测】系统字号调到特大时，左导航项文字**省略**而不是换行（导航不被撑成两行高度，⌘K 提示条与角标不被挤走）。
  4. 【可观测】a11y 基线：Tab 键能走到主要交互控件且有可见焦点环；聊天页当前会话条目带 `aria-current="page"`；确认弹窗有可读的对话框语义与带 `aria-label` 的关闭按钮。
  5. 【可核对】`grep -n "nm-card\|nm-btn\|nm-input\|nm-inset\|nm-outset\|nm-popover\|nm-tag" src/ui/main.css` 命中——nm 组件类集合在样式表一处定义，深浅两套自动生效。
  6. 【可核对】`grep -n "wmessage-theme" src/theme.ts` 命中——主题三态持久化在 localStorage，取值 light / dark / system。
  7. 【可核对】`grep -n "lucide-react" src/App.tsx` 命中——图标走 lucide 统一图标集，不用手绘字符图标。
- **证据**：截图（浅色/深色/跟随系统三态 + 四个页面各一张 + 特大字号导航 + 焦点环）
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（本卡无审计事件，改查 `grep -n "nm-card" src/ui/main.css`：nm 类不在 = 卡片过期）
- **缺口补验**：无实机卡：nm 组件类、深浅色三态、a11y 基线未验

### ACC-P-10 · 流式恢复链与等待闸加固（2026-10-30 批次）

- **SPEC**：6-115
- **层级**：L2 实机
- **历史卡**：—（无：本批为 2026-10-30 新增防线）
- **前置**：应用已启动；已配置可用模型；准备一个会长时间运行的任务（如派发子 agent 做调研）
- **步骤**：
  1. 任务执行会话围观中：等回复流到一半，切到别的会话再切回。
  2. 派一个子 agent 长任务，主 agent `check_subagent(waitMs=600000)` 等待期间按停止。
  3. 触发一个危险操作确认弹窗，等 55s 后刷新窗口（⌘R / webview 重载）。
  4. 制造一条无终态的死轮（如拔掉模型网络等回复卡住——如不可控可跳过此步只验看门狗常量存在）。
- **判据**：
  1. 【可观测】步骤 1 切回后，已流出的正文还在气泡里（不丢半截），回合收尾后正文以库为准整段替换。
  2. 【可观测】步骤 2 按停止后 ≤1s 内界面停止「正在回复」（不再等满等待预算）。
  3. 【可观测】步骤 3 重载后确认弹窗被还原，倒计时显示**后端剩余秒数**（≤5s 而不是重置的 60s）；到期前点击有效。
  4. 【可观测】步骤 4（若可制造）660s 无流式事件后气泡转为失败提示、停止键消失，会话可继续发消息（不永久「回复中」）。
  5. 【可核对】`grep -n "CHECK_STOP_SLICE_MS" src-tauri/src/bot_orchestrator.rs` 命中（等待切片常量 = 500ms）。
  6. 【可核对】`grep -n "STREAM_DEAD_IDLE_MS" src/components/ChatPanel/constants.ts` 命中（看门狗窗口 = 660s）。
  7. 【可核对】`grep -rn "remaining_secs" src-tauri/src/bot_slash.rs` 命中（重查还原带后端剩余时长）。
  8. 【可核对】`grep -n "sse_data_batch_truncated" src-tauri/src/bot_model_loop/sse.rs` 命中（攒批 2MB 上限审计事件）。
- **证据**：录屏（切回不丢正文 + 停止秒级生效 + 重载还原倒计时）+ 审计日志截图（`llm.sse_data_batch_truncated` / `model_loop_panic` 如有）
- **失败处置**：先分清「应用坏了」还是「卡片过期」——`grep -n "CHECK_STOP_SLICE_MS" src-tauri/src/bot_orchestrator.rs`：常量不在 = 卡片过期；常量在但停止仍拖满预算 = 应用侧回归
- **缺口补验**：无实机卡：回放接回 / 看门狗判死 / 确认还原 / 停止响应均未实机验证

---

### ACC-P-15 · 检查更新（Windows）

- **SPEC**：6-120
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：—
- **前置**：组件面即测即验；实机面需 GitHub Releases 上已发布首个带 latest.json 的版本
- **步骤**：
  1. 组件面（L1）：跑 `npx vitest run src/components/SettingsPage/UpdateCard.test.tsx`（五用例：UA 判定 / 非 Windows 不渲染 / 无新版显已最新 / 有新版下载→relaunch / 失败显式报错不冒充最新）。
  2. 实机（L2）：Windows 装旧版 → GitHub 发布更高版本 release（安装器改固定资产名 + latest.json）→ 设置页「通用设置」点「检查更新」。
  3. 看下载进度 → passive 安装进度条 → 应用自动重启 → 卡上版本号变新。
  4. 断网点「检查更新」→ 看错误文案。
- **判据**：
  1. 【可核对】组件五用例全绿。
  2. 【可观测】实机：发现新版本 → 下载装完自动重启 → `getVersion` 变新。
  3. 【可观测】断网/限流 → 红字错误文案，**不显示「已是最新」**。
  4. 【可核对】`grep -n "pubkey" src-tauri/tauri.conf.json` 命中（验签公钥内置）；`grep -n "createUpdaterArtifacts" src-tauri/tauri.windows.conf.json` 命中（签名产物开关只在 Windows 侧，mac 构建不需签名密钥）。
- **证据**：vitest 输出 + 实机录屏 + GitHub release 页截图
- **失败处置**：先查 latest.json 的 `signature` 字段是否为 .sig 原文（多行内容 JSON 转义）与 asset 是否用固定资产名；再查构建时是否导出了 `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`。
- **缺口补验**：实机更新链路未跑（需首个 release）；验签失败路径（篡改安装器）未制造。

### ACC-P-16 · 用户指令文件

- **SPEC**：6-121
- **层级**：L1（自动化；实机端到端随下一轮实机批次抽验）
- **历史卡**：—
- **前置**：无（纯本地文件与提示词装配，不依赖外部服务）
- **步骤**：
  1. 组件面：跑 `npx vitest run src/components/SettingsPage/InstructionsCard.test.tsx`（四用例：载入回填+字数 / 保存带完整 content / 超限禁保存钮并显提示 / 保存失败人话展示）。
  2. 后端面：跑 `cargo test --lib -- bot::instructions prompt_builder`（六用例：短文原样 / 字符边界截断+标注 / 恰好上限不截 / 缺文件空串 / 读回 / 超限载入截断；槽位序测试断言 UserInstructions 落 Base 与 GenDir 之间、VerifierRetryTail 恒末位）。
  3. 对拍：`grep -n "UserInstructions" src-tauri/src/prompt_builder.rs` 命中；`grep -n "load_user_instructions" src-tauri/src/bot_chat/flow.rs` 命中（装配接线在案）。
- **判据**：
  1. 【可核对】保存超 16,000 字被拒（`instructions_set` 返回人话错误，不落盘）；载入侧超限截断尾部带「（已截断）」标注。
  2. 【可观测】审计 `instructions.updated`（保存）/ `instructions.truncated`（进程内首次截断）在 bot.log 可查；截断告警不逐回合刷屏。
  3. 【可核对】文件缺失或为空时 system 不含该槽（空串跳过，无多余文案）。
- **证据**：vitest / cargo 输出 + bot.log 审计行
- **失败处置**：先查 data_dir 下 instructions.md 是否被外部占用（读错误按空指令处理、留 warn 日志）；再查 PromptSlot 槽序是否被人改动（槽位序测试会红）。
- **缺口补验**：实机「设置页编辑→发消息→模型遵从指令」端到端未验；截断标注在真机 system 的呈现未验。

### ACC-P-17 · 回合级快照回滚

- **SPEC**：6-122
- **层级**：L1（自动化；实机端到端随下一轮实机批次抽验）
- **历史卡**：—
- **前置**：无（纯本地文件、SQLite 与系统回收站）
- **步骤**：
  1. 后端面：跑 `cargo test --lib -- db::trace`（rollback 三支五例：modify 恢复/漂移拒/篡改快照拒 + delete 恢复与防覆盖 + create 撤销进回收站与漂移拒；回合聚合一例 modify+delete 恢复 create 漂移跳过；孤儿清扫一例 live/orphan-old/orphan-new 三态）。
  2. 组件面：`grep -n "onTurnRollback" src/components/ChatPanel/ChatPanel.tsx src/components/ChatPanel/MessageList.tsx` 命中（弹层接线在案）；`grep -n 'c.kind === "delete"' src/components/TracePanel/TracePanel.tsx` 命中（三 kind 判定）。
  3. 对拍：`grep -n "bot_turn_rollback" src-tauri/src/lib.rs` 命中（命令注册在案）。
- **判据**：
  1. 【可核对】delete 回滚：目标被重建 → 拒且重建内容原样保留；已删 → 快照按字节恢复全文。
  2. 【可核对】create 撤销：内容与 after_sha 一致 → 移入系统回收站；漂移 → 拒。
  3. 【可核对】回合级：files=true 时同 path 取最新条、漂移跳过不中断整批（restored/skippedDrift 报告）；忙时（在途回合）拒绝。
  4. 【可观测】审计 `turn.rollback`（restored/skipped_drift/failed）与 `checkpoint.swept` 在 bot.log 可查。
- **证据**：cargo/vitest 输出 + bot.log 审计行
- **失败处置**：先查 checkpoints 目录是否被人为清空（快照缺失=不可恢复，按 deny 设计）；再查 ChatGuard 忙闸误判（`chat_guard_is_held` 转正后的锁表访问）。
- **缺口补验**：实机三态弹层端到端（撤回复/连消息撤/只撤文件）未验；manage_file move 双凭证的实机回滚未验；Windows 回收站撤销（trash crate 平台差异）未验。

### ACC-P-18 · 工具分组按需挂载

- **SPEC**：6-123
- **层级**：L1（自动化；实机端到端随下一轮实机批次抽验）
- **历史卡**：—
- **前置**：灰度开关默认关（deferredToolGroups 未配置 = 行为零变化）
- **步骤**：
  1. 后端面：跑 `cargo test --lib -- bot::registry::groups`（六例：组清单与 TOOLS_TABLE 一一对应 / 量化数字（全量 62,624 / 默认 49,366）/ 默认清单排除休眠与子 agent 专属且含 activate_tools / 激活后组工具挂载其他休眠组仍不挂 / 兜底闸三态（关=直通、开未激活=拒、激活后放行）/ 非法组与空 groups 拒）。
  2. 量化复核：`cargo test --lib quantify_group_sizes -- --nocapture` 打印全量/默认/各休眠组字符量。
  3. 对拍：`grep -n "deferred_gate" src-tauri/src/bot/dispatch.rs` 命中（0.57 兜底闸接线在案）；`grep -n "deferred_tool_groups" src-tauri/src/bot/config/types.rs` 命中（灰度开关字段在案）。
- **判据**：
  1. 【可核对】开关关时 tools_json_for 返回与全量逐字节相同（默认行为零变化）。
  2. 【可核对】开关开：休眠组工具不在 schema；dispatch 拒幻觉调用且文案含 activate_tools；activate_tools 激活后下一轮 schema 含该组且 dispatch 放行。
  3. 【可核对】activate_tools 只接受 desktop/services/apps 三组名；子 agent 会话白名单链路不受影响（0.55 先拒）。
- **证据**：cargo 输出 + 量化数字
- **失败处置**：先查组清单覆盖完整性测试（新增工具忘入组会红）；再查开关同步（load_config / update_config_file 两处都写 set_groups_enabled）。
- **缺口补验**：实机「开关开→对话→activate_tools→桌面工具实际可用」端到端未验；激活瞬间缓存 miss 的 input 成本实测未做；设置页 UI 开关（方案 P-3 手改 config 口径）未做。

### ACC-P-19 · MCP elicitation 桥接

- **SPEC**：6-124
- **层级**：L1（自动化；实机端到端随下一轮实机批次抽验）
- **历史卡**：—
- **前置**：per-server `allowElicitation` 默认 false（默认行为=rmcp 自动 Decline，零变化）；需要一个会发 elicitation 的 HTTP MCP server
- **步骤**：
  1. 后端面：跑 `cargo test --lib -- bot::mcp::elicit`（三例：多字段与未知字段类型 Decline / URL 模式与无在途调用 Decline / elicit_respond 唤醒 waiter 且同 qid 二次应答报不存在）。
  2. 组件面：`grep -n "elicit_respond" src/components/ChatPanel/AskCard.tsx src-tauri/src/lib.rs` 命中（应答分叉 + 命令注册在案）；`grep -n "allow_elicitation" src-tauri/src/bot/mcp/config.rs src-tauri/src/bot/mcp/manager.rs` 命中（开关 + handler 注入在案）。
  3. 对拍：`grep -n "source.*mcp_elicit" src-tauri/src/bot/mcp/elicit.rs src/components/ChatPanel/askStore.ts` 命中（来源标记两侧一致）。
- **判据**：
  1. 【可核对】开关关：elicitation 到达 → 自动 Decline（与现状逐字节一致）。
  2. 【可核对】开关开：在途调用期间 elicitation → ask 卡弹出（含 server 名、单字段）；60s 超时 → Decline；主窗口不可见 → 立即 Decline。
  3. 【可核对】应答值不出现在 bot.log / 审计事件（只回 `[MCP 应答，值不落日志]` 占位）。
  4. 【可核对】字段类型映射：boolean 走「允许/不允许」按钮、enum 走选项按钮、number/integer 严格解析失败 → Decline。
- **证据**：cargo 输出 + AskCard 截图（实机）
- **失败处置**：先查在途登记（execute_mcp_direct 的 ActiveCallGuard 是否泄漏——panic 路径由 RAII 兜底）；再查 rmcp SEP-2260 关联校验（协议 ≥2026-07-28 时无在途关联的请求 rmcp 内部直接 invalid_params，属预期）。
- **缺口补验**：实机端到端（真 server elicitation → 卡 → 应答回传）未验；stdio 服务器 elicitation 未接（范围外）；URL 模式未接（范围外）。
