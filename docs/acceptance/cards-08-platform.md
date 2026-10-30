# ACC-P · 数据、集成与平台（L2 实机卡）

> 覆盖 `SPEC.md`「6. 数据、集成与平台」9 条功能（6-106 ~ 6-114），对应矩阵 `docs/acceptance/matrix.csv` 的 D6 域。
> 判据写法与证据规范见 `docs/ACCEPTANCE-PLAN-2026-10-30.md`；审计事件名取自 `docs/acceptance/event-names.md` 白名单。
> 数据目录（macOS）：`~/Library/Application Support/com.renshi.wmessage/`（便携模式随 exe 走，见 ACC-P-03）。下文的 `<data_dir>` 指该目录。
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

### ACC-P-03 · 便携模式：数据库随 exe 走

- **SPEC**：6-108
- **层级**：L1+L2（L1 自动化 + L2 实机）
- **历史卡**：W01（`docs/realmachine-mutation-test-2026-10-23.md`）、W10、S31（`docs/realmachine-smoke-test-2026-10-25.md`）
- **前置**：已有一个能跑的构建产物（exe / .app）；有一个可写的空目录用来放它
- **步骤**：
  1. 把应用拷进一个空的可写目录，双击启动，等它起来。
  2. 看这个目录里出现了什么；再看原来的 app_data 目录，库文件的修改时间有没有变。
  3. 在应用里跑一轮对话，确认数据（库、日志、产物）都写在 exe 旁边。
  4. 把应用挪进一个**不可写**的目录（macOS：`/Applications` 下或 `chmod a-w` 的目录）再启动，看它把数据落在哪、日志里有没有兜底说明。
  5. 把应用从 zip 里直接双击运行（exe 落系统 temp），看数据目录落点。
- **判据**：
  1. 【可观测】步骤 1：应用在 exe 同目录建数据——同目录出现 `<data_dir>/wmessage.db` 与 `AI_Gen_Files/`；原 app_data 目录里的库文件修改时间不变。
  2. 【可观测】步骤 3：一轮对话后，`<data_dir>/wmessage.db`、`<data_dir>/bot.log` 与 `AI_Gen_Files/` 都落在 exe 同目录，app_data 目录里没有新增本轮数据。
  3. 【可观测】步骤 4：exe 目录不可写时，数据回落到 app_data 目录（`~/Library/Application Support/com.renshi.wmessage/`），启动日志里有一条说明兜底原因的 WARN。
  4. 【可观测】步骤 5：zip 内直接双击运行时不在系统 temp 下建数据目录，退化到 app_data 目录，且日志里记一条 WARN 说明「疑似压缩包内直接双击运行，请解压后再运行」。
  5. 【可核对】exe 旁已有 `<data_dir>/wmessage.db` 或 `AI_Gen_Files` 时强制便携锚定（写探针失败也不翻转）；`grep -n "fn probe_dir_cached_in\|fn probe_dir\b" src-tauri/src/paths.rs` 命中——数据目录判定只有一处，首次探测结果钉死、后续条件变化也不翻转。
  6. 【可核对】`grep -n "RUNTIME_SUBDIR" src-tauri/src/paths.rs` 命中且值为 runtime——便携模式下 runtime/ 子目录（API token 等）同样随 exe 走。
- **证据**：截图（exe 目录文件清单 + app_data 目录文件清单）+ 启动日志片段
- **失败处置**：先分清「应用坏了」还是「卡片过期」——用 `grep -F "<事件名>" src-tauri/src` 确认事件名还在源码里（本卡无专属审计事件，改查 `grep -n "fn probe_dir" src-tauri/src/paths.rs`：判定逻辑不在 = 卡片过期；逻辑在但落点不对 = 应用侧回归）
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
- **判据**：
  1. 【可观测】步骤 2：产物文件上**查不到「来自互联网」标记**——Windows 上 `Get-Item -Path "<产物>" -Stream *` 的列表里没有 `Zone.Identifier` 这一条；macOS 上不存在该备用流。
  2. 【可核对】`grep -n "ZONE_STREAM" src-tauri/src/platform/zone_id.rs` 命中且值为 `Zone.Identifier`——只删这一条流，主数据流与其余流不动。
  3. 【可核对】`grep -rn "strip_zone_identifier" src-tauri/src/py/` 命中五个文档产物出口（word / word_revisions / excel / pdf / ppt）加 Word 模板上传与 run_python 产物搬运的调用点——六个出口走同一个收口。
  4. 【可观测】步骤 3/4：**用户自己的原文件不动**——源文件的「来自互联网」标记在处理后仍在（标记是用户的安全策略，app 不擅自改）。
  5. 【可观测】剥不掉时只记审计不阻断：命令仍然成功、文件仍然交付，模型侧收到的是「文件本身是好的」这一事实，不是失败。
  6. 【可核对】`grep -n "fn strip_zone_identifier" -A 8 src-tauri/src/platform/zone_id.rs` 命中，且非 Windows 分支是空实现（macOS 上不做任何事，也不报错）。
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
