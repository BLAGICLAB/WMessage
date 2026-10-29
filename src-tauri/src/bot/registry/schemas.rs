//! 工具 schema 字面量与工具表：65 个 `SCHEMA_*` 常量 + `TOOLS_TABLE` 接线 +
//! tools JSON 拼装与子 agent profile 白名单。

use std::sync::OnceLock;

use super::handlers::*;
use super::ToolDef;

// schema 常量（baseline 前缀一致）
pub const SCHEMA_QUERY_TASKS: &str = r##"{"type":"function","function":{"name":"query_tasks","description":"查询任务卡：不传 query=列清单（view 默认 active 未完成）；传 query=按关键词检索（匹配标题/备注/标签/子任务，此时 view 默认 all 全库）；定位任务不确定时先调本工具确认","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"关键词，可选；不传=列清单"},
    "view":{"type":"string","enum":["active","done","archived","trash","all"],"description":"视图范围，可选：active=未完成 / done=已完成未归档 / archived=已归档 / trash=回收站 / all=除回收站外全部；默认随 query 自动定（无 query=active，有 query=all）"},
    "tag":{"type":"string","description":"按标签过滤，可选"},
    "limit":{"type":"integer","description":"最多返回条数，可选，默认 50，上限 200"}
}}}}"##;
pub const SCHEMA_QUERY_SINGLE_TASK: &str = r##"{"type":"function","function":{"name":"query_single_task","description":"按 id 查询单张任务卡完整详情（标题/列/截止/备注/子任务/标签/绑定文件 + 创建时间/依赖 + 归档/删除状态指示；白名单单点，区别于 query_tasks 批量清单与关键词检索；不要为凑清单循环调用本工具，批量场景一律用 query_tasks）","parameters":{"type":"object","properties":{"id":{"type":"string","description":"任务卡 UUID"}},"required":["id"]}}}"##;
pub const SCHEMA_CREATE_TASK: &str = r##"{"type":"function","function":{"name":"create_task","description":"新建任务","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"任务标题"},
    "note":{"type":"string","description":"备注，可选"},
    "due":{"type":"string","description":"截止时间，YYYY-MM-DD 或 YYYY-MM-DDTHH:mm（T 分隔，空格分隔不合法），可选；示例：写 2026-10-24 或 2026-10-24T09:00，写 2026-10-24 09:00 或只写 2026-10 都会被拒。due 只触发截止提醒，到点不会执行任务——「到点自动执行」目前只支持用户在定时页建作业（schedule_reminder 到点也只说话不执行）；用户要定时执行时如实说明这一限制，不要建 due 卡就声称会自动运行"},
    "column":{"type":"string","enum":["todo","doing"],"description":"状态列，默认 todo；示例：只有 todo / doing 两个值（archived / done 不合法，归档与完成另有专门动作）"},
    "files":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"isDir":{"type":"boolean"}}},"description":"追加绑定文件/文件夹（可选，追加不覆盖；最多 10 个，每卡一个文件夹，绑新文件夹替换旧的；安全约束：仅允许 AI_Gen_Files 目录内的已存在文件，其余会被丢弃；要绑其它文件请引导用户用 bind_file 手选）"}
  },"required":["title"]}}}"##;
pub const SCHEMA_COMPLETE_TASK: &str = r##"{"type":"function","function":{"name":"complete_task","description":"完成任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配；任务卡带【验收标准】且未逐条达标时不要调用）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id（来自用户消息的 [已选任务] 引用块或 query_tasks 输出），可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用；示例：只能完成未完成的卡——已归档或已删除的卡会报「未找到」，先 query_tasks 确认它还在哪一档"}
  },"required":[]}}}"##;
pub const SCHEMA_DELETE_TASK: &str = r##"{"type":"function","function":{"name":"delete_task","description":"删除任务到回收站（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"}
  },"required":[]}}}"##;
pub const SCHEMA_EDIT_TASK: &str = r##"{"type":"function","function":{"name":"edit_task","description":"编辑任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配；改标题/备注/截止时间/标签/状态列/执行模型/归属人，空串清字段）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时用于定位任务"},
    "newTitle":{"type":"string","description":"新标题，可选"},
    "note":{"type":"string","description":"新备注，可选；空串清除"},
    "due":{"type":"string","description":"新截止时间，可选；空串清除；格式 YYYY-MM-DD 或 YYYY-MM-DDTHH:mm（T 分隔）"},
    "column":{"type":"string","enum":["todo","doing","done"],"description":"新状态列，可选；示例：只有 todo / doing / done 三个值（archived 不合法）"},
    "tags":{"type":"array","items":{"type":"string"},"description":"新标签列表，可选；空数组清除"},
    "files":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"isDir":{"type":"boolean"}}},"description":"追加绑定文件/文件夹（可选，追加不覆盖已有绑定；最多 10 个，每卡一个文件夹，绑新文件夹替换旧的；空数组 = 无操作，本工具不能清空绑定；安全约束：仅允许 AI_Gen_Files 目录内的已存在文件，其余会被丢弃）"},
    "model":{"type":"string","description":"每卡执行模型（执行这张卡时覆盖全局激活模型），可选；空串清除=恢复跟随全局"},
    "owner":{"type":"string","description":"归属成员名或 personId（任务图谱按成员过滤用），可选；空串=归属本人；名字歧义时会返回候选名单，先向用户确认再重试"}
  },"required":[]}}}"##;
pub const SCHEMA_ADD_SUBTASK: &str = r##"{"type":"function","function":{"name":"add_subtask","description":"给任务添加子任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"},
    "text":{"type":"string","description":"子任务内容"}
  },"required":["text"]}}}"##;
pub const SCHEMA_TOGGLE_SUBTASK: &str = r##"{"type":"function","function":{"name":"toggle_subtask","description":"勾选/取消勾选子任务（任务用 taskId 优先；子任务用 subtaskId 精确优先，无 id 时按内容关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "subtaskId":{"type":"string","description":"子任务 id（query_single_task 输出），可选，优先于 text"},
    "text":{"type":"string","description":"子任务内容关键词，无 subtaskId 时必填"}
  },"required":[]}}}"##;
pub const SCHEMA_REMOVE_SUBTASK: &str = r##"{"type":"function","function":{"name":"remove_subtask","description":"删除单条子任务（彻底移除，区别于 toggle_subtask 的取消勾选；任务用 taskId 优先；子任务用 subtaskId 精确优先，无 id 时按内容关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "subtaskId":{"type":"string","description":"要删除的子任务 id（query_single_task 输出），可选，优先于 text"},
    "text":{"type":"string","description":"要删除的子任务内容关键词，无 subtaskId 时必填"}
  },"required":[]}}}"##;
pub const SCHEMA_READ_TEXT_FILE: &str = r##"{"type":"function","function":{"name":"read_text_file","description":"读取本地文本文件内容（白名单目录内直接读，白名单外自动弹窗请用户授权；≤500 行的文件不传 limit 一次读完，offset/limit 分页只用于更大的文件；Office/PDF 用 extract_document，图片用户会直接发图）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径（支持 ~ 开头）"},
    "offset":{"type":"integer","description":"起始行号，从 1 开始，可选"},
    "limit":{"type":"integer","description":"读取行数，默认 500，最多 2000，可选"}
  },"required":["path"]}}}"##;
pub const SCHEMA_OCR_IMAGE: &str = r##"{"type":"function","function":{"name":"ocr_image","description":"本地 OCR 识别图片文字，逐行返回（白名单目录内直接识别，白名单外自动弹窗请用户授权；隐私红线：图片仅在内存处理、绝不上传外网——macOS 用系统 Vision，Windows 用本地 PP-OCRv6 模型）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"图片绝对路径（支持 ~ 开头；仅本地文件，不接受网络地址）"}
  },"required":["path"]}}}"##;
pub const SCHEMA_GREP_FILES: &str = r##"{"type":"function","function":{"name":"grep_files","description":"按正则搜索本地文件内容，返回 path:行号:内容（最多 50 条；白名单目录内直接搜，白名单外自动弹窗请用户授权）。dir 可以是目录（递归搜其下所有文件），也可以是单个文件的绝对路径（只搜那一个文件）","parameters":{"type":"object","properties":{
    "pattern":{"type":"string","description":"正则表达式（非法正则自动按字面量搜）"},
    "dir":{"type":"string","description":"搜索目录，可选，缺省搜第一个白名单目录；也可传单个文件的绝对路径，此时只搜该文件"},
    "glob":{"type":"string","description":"文件名过滤，如 *.rs，可选"},
    "max":{"type":"integer","description":"最多返回条数，默认 50，可选"},
    "context":{"type":"integer","description":"命中行上下文行数，可选，0-5，默认 0（显示匹配行前后各 N 行，便于读懂语境）"}
  },"required":["pattern"]}}}"##;
pub const SCHEMA_LIST_FILES: &str = r##"{"type":"function","function":{"name":"list_files","description":"列出本地目录内的文件/子目录（递归 ≤5 层，最多 200 条；可用 pattern 按文件名过滤；白名单目录内直接列，白名单外自动弹窗请用户授权）","parameters":{"type":"object","properties":{
    "dir":{"type":"string","description":"目录绝对路径（支持 ~ 开头）"},
    "pattern":{"type":"string","description":"文件名过滤，如 *.pdf 或 报告*，可选"}
  },"required":["dir"]}}}"##;
pub const SCHEMA_LINK_FILE_TO_TASK: &str = r##"{"type":"function","function":{"name":"link_file_to_task","description":"把 AI_Gen_Files 目录内的文件或文件夹绑定到任务卡。任务卡执行流程（🤖 按钮 / ⏰ 定时 / 📦 批量）内：登记到产物清单，流程结束、任务完成时落「通知中心」消息让用户勾选绑定（默认全选，每个文件绑一次）；任务未完成、中断、只有中间产物都不产生待绑定消息。普通对话内：直接绑定落卡，但必须指定一张已存在的任务卡（taskId 优先；title 需精确匹配唯一一张卡，本工具不能新建卡）。绑定一律追加，不覆盖卡上已有绑定；每卡最多一个文件夹（绑新文件夹会替换旧的），同一路径重复绑无效。","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title；普通对话内必须能命中已存在的卡"},
    "title":{"type":"string","description":"任务标题，无 taskId 时使用，须精确匹配一张已存在的卡"},
    "path":{"type":"string","description":"产物文件/文件夹绝对路径，必须在 AI_Gen_Files 目录内且已存在"},
    "kind":{"type":"string","enum":["final","intermediate"],"default":"final","description":"final=最终产物，intermediate=中间产物。仅任务执行流程的登记分支生效（参与流程结束的待绑定汇总）；普通对话直接绑定，本参数无效果。"}
  },"required":["path"]}}}"##;
pub const SCHEMA_EXTRACT_DOCUMENT: &str = r##"{"type":"function","function":{"name":"extract_document","description":"提取文档内容（不传 path 时弹系统选择框由用户选 Word/Excel/PPT/PDF；传 path 时直接读取该文件，如任务卡的绑定文件；长文档用 offset 参数续读后续部分；扫描版/图片型 PDF 文本层为空时自动转图走本地 OCR 兜底，前 20 页）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径，可选"},
    "offset":{"type":"integer","description":"字符偏移（可选，默认 0；返回里带『已截断』提示时用提示的 offset 值续读）"},
    "limit":{"type":"integer","description":"本页字符数（可选，默认 30000，上限 60000）"}
  }}}}"##;
pub const SCHEMA_CREATE_WORD: &str = r##"{"type":"function","function":{"name":"create_word","description":"生成 Word 文档到 AI_Gen_Files（润色后的文本用这个落地；不覆盖任何已有文件）。生成是否成功以本工具返回为准：返回 Err 就是没生成，不得向用户声称已生成；成功时必须把返回的完整路径告知用户","parameters":{"type":"object","properties":{"template":{"type":"string","description":"模板名（设置页「Word 模板」已上传的）。用户指定了模板就传；未指定自动用默认模板（未设置默认则内置版式）；存在多个模板且用户意图不明时，先用 ask_user 问用户用哪个"},
    "title":{"type":"string","description":"文档标题，可选"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表，每段一个字符串"},
    "tables":{"type":"array","description":"可选：表格列表，按顺序追加在段落之后；每个表 rows 二维数组、第一行当表头加粗","items":{"type":"object","properties":{
      "title":{"type":"string","description":"表格标题，可选"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}
    },"required":["rows"]}},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"},
    "images":{"type":"array","items":{"type":"string"},"description":"要插入的图片绝对路径列表，可选（仅 AI_Gen_Files 目录内的已存在图片，其余被丢弃并提示；按顺序插在正文之后、表格之前）"}
  },"required":["paragraphs"]}}}"##;
pub const SCHEMA_CREATE_WORD_REVISIONS: &str = r##"{"type":"function","function":{"name":"create_word_revisions","description":"生成带修订标记（修订模式）的 Word 到 AI_Gen_Files：在原文档副本上就地对比原文与润色后的段落打 Word 原生 track changes（保留原文格式/字体），可在 Word 审阅中逐条接受/拒绝（引擎：.NET OpenXML 优先，Python 兜底）","parameters":{"type":"object","properties":{
    "originalPath":{"type":"string","description":"原文 Word 路径（extract_document 返回的 [文档路径]）"},
    "original":{"type":"array","items":{"type":"string"},"description":"原文行列表（提取被截断时必须传，保证对比范围一致），可选"},
    "revised":{"type":"array","items":{"type":"string"},"description":"润色后的段落列表"},
    "title":{"type":"string","description":"文档标题，可选"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["revised"]}}}"##;
pub const SCHEMA_CREATE_EXCEL: &str = r##"{"type":"function","function":{"name":"create_excel","description":"生成 Excel 到 AI_Gen_Files（单元格以 = 开头会写入原生公式如 =SUM(A1:A10)）。生成是否成功以本工具返回为准：返回 Err 就是没生成，不得向用户声称已生成；成功时必须把返回的完整路径告知用户","parameters":{"type":"object","properties":{
    "sheets":{"type":"array","items":{"type":"object","properties":{
      "name":{"type":"string"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}}},
    "description":"工作表列表：name 表名、rows 二维数组"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["sheets"]}}}"##;
pub const SCHEMA_CREATE_PPT: &str = r##"{"type":"function","function":{"name":"create_ppt","description":"生成排版统一的 PPT 到 AI_Gen_Files（版式/配色/字体随模板：设置页可导入 .pptx 母版设默认；未导入时用内置专业母版，封面/章节页深底、内容页浅底、中文雅黑）。多版式：封面/目录/章节页/内容页/表格页/结束页；要点超 5 条自动拆页。需要设计感更强的自由版式（图表/双栏/大数字）时，若已安装 ppt 设计类技能（见技能清单）优先用技能。生成是否成功以本工具返回为准：返回 Err 就是没生成，不得向用户声称已生成；成功时必须把返回的完整路径告知用户","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"演示文稿主标题"},
    "template":{"type":"string","description":"模板名（设置页已导入的 .pptx 母版名，不含扩展名），可选；缺省用默认模板，未设默认时用内置母版。用户给过公司模板/指定过模板名时用"},
    "slides":{"type":"array","description":"幻灯片列表，按展示顺序；每页一个 type","items":{"type":"object","properties":{
      "type":{"type":"string","enum":["cover","toc","section","content","table","closing"],"description":"页面类型：cover 封面（title+subtitle）/ toc 目录（items 列表，缺省自动取各页标题）/ section 章节分隔页 / content 内容要点页 / table 表格页（rows 二维数组首行表头）/ closing 结束页"},
      "title":{"type":"string","description":"页面标题；content 页标题写结论式短句（如「营收同比 +23%」），禁「介绍」「概述」"},
      "subtitle":{"type":"string","description":"副标题（cover/section/closing 用）"},
      "bullets":{"type":"array","items":{"type":"string"},"description":"要点列表（content 页；硬规范 ≤5 条/页、每条 ≤40 字，超限自动拆页）"},
      "items":{"type":"array","items":{"type":"string"},"description":"目录条目（toc 页）"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}},"description":"表格数据（table 页；第一行是表头）"},
      "notes":{"type":"string","description":"演讲者备注/讲稿（本页口播内容，放这里不放正文），可选"}
    },"required":["type","title"]}},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["slides"]}}}"##;
pub const SCHEMA_CREATE_PDF: &str = r##"{"type":"function","function":{"name":"create_pdf","description":"生成 PDF 到 AI_Gen_Files（中文支持，自动分页；自带页眉=文档标题与页脚页码）。生成是否成功以本工具返回为准：返回 Err 就是没生成，不得向用户声称已生成；成功时必须把返回的完整路径告知用户","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"文档标题，可选（同时用作每页页眉）"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表；#/##/### 前缀标记一/二/三级标题（如「## 市场分析」）"},
    "images":{"type":"array","items":{"type":"string"},"description":"要插入的图片绝对路径列表，可选（仅 AI_Gen_Files 目录内的已存在图片，其余被丢弃并提示；按顺序插在正文之后、表格之前；等比缩放到版心宽）"},
    "tables":{"type":"array","description":"可选：表格列表，按顺序追加在段落之后；每个表 rows 二维数组、第一行当表头加粗","items":{"type":"object","properties":{
      "title":{"type":"string","description":"表格标题，可选"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}
    },"required":["rows"]}},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["paragraphs"]}}}"##;
pub const SCHEMA_RUN_PYTHON: &str = r##"{"type":"function","function":{"name":"run_python","description":"执行 Python 代码（资源受限：CPU/内存/时长限额 + 独立临时目录，无文件系统隔离；默认超时 60s。默认需用户在设置页开启 Python 编程，授权模式为 yolo 时免开关）。脚本较长时先用 write_file 把代码落盘，再用 scriptPath 执行：出错只改出错的那几行重跑即可，不必把整段代码重写一遍。","parameters":{"type":"object","properties":{
    "code":{"type":"string","description":"要执行的 Python 代码，print 输出返回给用户"},
    "scriptPath":{"type":"string","description":"可选：已落盘的 Python 脚本路径（白名单内，优先用 AI_Gen_Files 下的路径）。给了它就执行该文件、不再用 code。长脚本先 write_file 落盘再这样调用；出错后用 edit_file 改出错那一行重跑，不必整段重写"},
    "timeoutSecs":{"type":"integer","description":"超时秒数（可选，默认 60；大计算可调大，上限 1800）"}
  },"required":[]}}}"##;
pub const SCHEMA_START_SERVICE: &str = r##"{"type":"function","function":{"name":"start_service","description":"启动一个受管长驻进程（node/deno/python 等白名单启动器）。进程归宿主托管：活过本次工具调用，可查可停，宿主退出时自动收干净。适合 node MCP server、dev server、watcher 这类需要持续运行的东西；一次性脚本用 run_python。启动器必须在白名单内（npx/bunx/uvx/pipx/node/deno/python/python3/docker/podman），不传 shell 字符串。默认需用户在设置页开启「允许机器人启动本地服务」，授权模式为 yolo 时免开关。服务继承宿主环境变量（含其中的密钥），只起自己写的代码","parameters":{"type":"object","properties":{
    "command":{"type":"string","description":"启动器名，必须在白名单内（npx/bunx/uvx/pipx/node/deno/python/python3/docker/podman）"},
    "args":{"type":"array","items":{"type":"string"},"description":"参数列表，逐个传入，不做 shell 解析"},
    "cwd":{"type":"string","description":"工作目录（绝对路径），受文件授权闸约束；省略则用宿主当前目录"},
    "name":{"type":"string","description":"用途标签，便于人识别，如「前端 dev server」；可省"},
    "readyPattern":{"type":"string","description":"就绪判据：stdout 出现该正则才算启动成功。省略=进程起来 200ms 后仍活着即算成功"},
    "readyTimeoutSecs":{"type":"integer","description":"等待就绪的超时秒数，默认 30，上限 120"}
  },"required":["command"]}}}"##;
pub const SCHEMA_STOP_SERVICE: &str = r##"{"type":"function","function":{"name":"stop_service","description":"停止一个受管服务。走降级信号阶梯：SIGINT → 100ms → SIGTERM → 400ms → SIGKILL，并递归回收脱离进程组的子孙进程。幂等：已停止的服务再停一次返回 stopped=false，不报错。","parameters":{"type":"object","properties":{
    "serviceId":{"type":"string","description":"start_service 返回的服务 id"}
  },"required":["serviceId"]}}}"##;
pub const SCHEMA_LIST_SERVICES: &str = r##"{"type":"function","function":{"name":"list_services","description":"列出宿主托管的受管服务：id / 名称 / 命令 / 状态 / pid / 已运行秒数 / 退出码 / 最近输出尾部。用于确认服务是否还活着、拿服务 id 去 stop_service。","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_AST_INSPECT: &str = r##"{"type":"function","function":{"name":"ast_inspect","description":"对 Python 源码做符号级静态探查（走本机 Python 沙箱 + 标准库 ast，零新增依赖：资源受限、独立临时目录、默认 60s 超时，沙箱无文件系统隔离故调用前已过白名单闸；需设置页开启「允许机器人执行 Python」，或授权模式为 yolo 时免开关）。只支持 .py 文件，非 .py 会明确拒绝并让你改用 grep_files / read_text_file，不会静默退化成文本检索。四种模式：symbols 出符号树（类/函数/方法 + 行号）、defs 定位定义、refs 找全部引用（区分裸名与属性访问 + 所在函数）、callgraph 出调用链（按名字匹配，非作用域解析；depth 层）。调用链跨文件现扫不缓存：单次最多扫 200 个文件 / 2MB，跳过 .git/__pycache__/node_modules/venv 等目录，目录里指向外部的 symlink 一律排除；触及上限时返回 truncated=true，此时结果不完整","parameters":{"type":"object","properties":{
    "op":{"type":"string","enum":["symbols","defs","refs","callgraph"],"description":"探查模式：symbols=符号树 / defs=定义位置 / refs=全部引用 / callgraph=调用链"},
    "path":{"type":"string","description":"目标 .py 文件绝对路径（支持 ~ 开头）；op=callgraph 时可传目录，扫该目录下所有 .py"},
    "symbol":{"type":"string","description":"符号名（op 为 defs/refs/callgraph 时必填；类方法可只写方法名，如 resolve_with_perm）"},
    "depth":{"type":"integer","description":"callgraph 展开层数（可选，默认 1，上限 3）"}
  },"required":["op","path"]}}}"##;
pub const SCHEMA_UNPACK_ARCHIVE: &str = r##"{"type":"function","function":{"name":"unpack_archive","description":"解压压缩包到目录（支持 zip / tar / tar.gz，格式按文件头自动识别，不看后缀）。逐条校验条目路径：带 .. / 绝对路径 / 盘符 / 软链的条目会被跳过并列出（防压缩包把文件写到目标目录外）；单文件 64MB、整包 256MB、条目 5000 条上限，超限中止并报「疑似压缩炸弹」；目标已存在的文件不覆盖（会列出来）。解出的文件按「已生成」计入产物，可如实说「已解压出 N 个文件」","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"压缩包路径（须在读白名单内：桌面/下载/文档 + 任务卡绑定文件夹 + 设置页 allowedDirs）"},
    "destDir":{"type":"string","description":"解压目标目录（须在可写目录内：AI_Gen_Files + 任务卡绑定文件夹 + allowedDirs，白名单外会弹确认）；不给则解到 AI_Gen_Files/解压/<包名>/"},
    "format":{"type":"string","enum":["auto","zip","tar","tar.gz"],"description":"压缩格式（可选，默认 auto 按文件头识别）"}
  },"required":["path"]}}}"##;
pub const SCHEMA_PACK_ARCHIVE: &str = r##"{"type":"function","function":{"name":"pack_archive","description":"把目录或文件打包成压缩包（zip / tar.gz）：目录会连同目录名一起打包（zip -r 口径），软链跳过不跟随也不写入链接本身；输出到 AI_Gen_Files 或指定路径。打包完成按「已生成」计入产物，向用户报告时必须附上完整路径","parameters":{"type":"object","properties":{
    "sourcePath":{"type":"string","description":"要打包的目录或文件（须在读白名单内）"},
    "outputPath":{"type":"string","description":"输出压缩包路径（须在可写目录内，白名单外会弹确认）；不给则 AI_Gen_Files/<源名>.zip"},
    "format":{"type":"string","enum":["zip","tar.gz"],"description":"压缩格式（可选）：不给按输出路径后缀判定（.tar.gz / .tgz → tar.gz，否则 zip）"}
  },"required":["sourcePath"]}}}"##;
pub const SCHEMA_MANAGE_FILE: &str = r##"{"type":"function","function":{"name":"manage_file","description":"文件复制 / 移动 / 删除。copy 复制（源保留）、move 移动（源消失，同卷直接重命名、跨卷复制后源入回收站）、delete 删除（进系统回收站，删前弹窗请用户确认，60s 无响应默认拒绝）。源路径须在读白名单内（桌面/下载/文档 + 任务卡绑定文件夹 + 设置页 allowedDirs），目标路径须在可写目录内（AI_Gen_Files + 任务卡绑定文件夹 + allowedDirs，白名单外弹确认）；目标已存在一律不覆盖；目录树里的符号链接既不跟随也不复制。copy/move 的目标按「已生成」计入产物，报告时必须附上完整目标路径","parameters":{"type":"object","properties":{
    "action":{"type":"string","enum":["copy","move","delete"],"description":"操作类型：copy=复制 / move=移动 / delete=删除（进回收站）"},
    "path":{"type":"string","description":"源路径（绝对路径，支持 ~ 开头；copy/move 的源、delete 的目标）"},
    "dest":{"type":"string","description":"目标路径（copy/move 必填；delete 不需要）"}
  },"required":["action","path"]}}}"##;
pub const SCHEMA_COPY_FILE_TO_CLIPBOARD: &str = r##"{"type":"function","function":{"name":"copy_file_to_clipboard","description":"把本地文件放进系统剪贴板：用户在微信 / 飞书 / 邮件等应用里按 Cmd+V（Windows Ctrl+V）就能把这个文件当附件粘出去——发文件给别人的最短路径。会覆盖剪贴板原内容。路径须在读白名单内（桌面/下载/文档 + 任务卡绑定文件夹 + 设置页 allowedDirs，白名单外弹授权）。一次可放多个文件（最多 20 个），用户在目标应用里粘贴出来就是这批文件","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"单个文件路径（绝对路径，支持 ~ 开头）；与 paths 二选一"},
    "paths":{"type":"array","items":{"type":"string"},"description":"多个文件路径（最多 20 个）；与 path 二选一"}
  }}}}"##;
pub const SCHEMA_SCHEDULE_REMINDER: &str = r##"{"type":"function","function":{"name":"schedule_reminder","description":"设一个到点提醒：到点用系统通知提醒用户（只说话，不建任务卡、不执行任何内容）。用户说「30分钟后提醒我 X」「明天9点提醒我 Y」「每天早上提醒我 Z」时用它。when 支持三种写法：绝对时间 2026-10-22T09:00、今天 HH:MM（今天已过则顺延到明天）、相对现在 +30m / +2h / +1d；repeat 决定重复频率（none 仅一次 / daily 每天 / weekly 每周 / monthly 每月，重复时按 when 的时刻与星期、日序锚定）。设完返回提醒 id，可配合 list_reminders / cancel_reminder 管理","parameters":{"type":"object","properties":{
    "text":{"type":"string","description":"提醒内容（要提醒用户什么，一句话）"},
    "when":{"type":"string","description":"提醒时间：绝对时间 2026-10-22T09:00 / 今天 HH:MM / 相对现在 +30m、+2h、+1d"},
    "repeat":{"type":"string","enum":["none","daily","weekly","monthly"],"default":"none","description":"重复频率：none=仅一次（默认）/ daily=每天 / weekly=每周 / monthly=每月"}
  },"required":["text","when"]}}}"##;
pub const SCHEMA_LIST_REMINDERS: &str = r##"{"type":"function","function":{"name":"list_reminders","description":"列出当前已设的全部提醒（含提醒 id、下次触发时间与启用状态），用于回答「我有哪些提醒」或拿到 cancel_reminder 需要的 id","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_CANCEL_REMINDER: &str = r##"{"type":"function","function":{"name":"cancel_reminder","description":"取消一条已设的提醒（按提醒 id，id 用 list_reminders 查）。取消后到点不再通知","parameters":{"type":"object","properties":{
    "id":{"type":"string","description":"提醒 id（list_reminders 返回）"}
  },"required":["id"]}}}"##;
pub const SCHEMA_LIST_WINDOWS: &str = r##"{"type":"function","function":{"name":"list_windows","description":"列出可接管的桌面窗口（应用名 + 窗口标题 + 窗口 id）。UI 自动化的第一步：先拿窗口 id，再对它做 inspect / click / type。只列有标题的窗口；需要先在设置页开启「允许机器人操作桌面界面」","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_ACTIVATE_WINDOW: &str = r##"{"type":"function","function":{"name":"activate_window","description":"把指定窗口切到前台（后续 click / type 都作用在这个窗口上）","parameters":{"type":"object","properties":{
    "windowId":{"type":"string","description":"窗口 id（list_windows 返回）"}
  },"required":["windowId"]}}}"##;
pub const SCHEMA_UI_INSPECT: &str = r##"{"type":"function","function":{"name":"ui_inspect","description":"读窗口里的控件树（角色 / 标题 / 当前值 / 是否可用 / 是否可直接触发），可按关键词过滤。点击与输入前先用它确认目标控件的 id","parameters":{"type":"object","properties":{
    "windowId":{"type":"string","description":"窗口 id（list_windows 返回）"},
    "query":{"type":"string","description":"可选：按控件标题 / 角色 / 标识过滤（不填=全部）"},
    "maxDepth":{"type":"integer","description":"可选：树深度上限，默认 6，最大 16（网页内容控件较深，默认看不到时可显式给 16）"}
  },"required":["windowId"]}}}"##;
pub const SCHEMA_UI_CLICK: &str = r##"{"type":"function","function":{"name":"ui_click","description":"点击窗口里的某个控件（按控件 id 或标题关键词定位）。点「发送 / 提交 / 支付 / 删除」这类按钮、以及没有标题的图标按钮（纸飞机/垃圾桶）前会弹窗请用户确认，用户拒绝或 60 秒无响应则不执行——真发出去的消息撤不回来，所以这一步必须让用户过目","parameters":{"type":"object","properties":{
    "windowId":{"type":"string","description":"窗口 id（list_windows 返回）"},
    "target":{"type":"string","description":"控件 id（ui_inspect 返回）或控件标题关键词"}
  },"required":["windowId","target"]}}}"##;
pub const SCHEMA_UI_TYPE: &str = r##"{"type":"function","function":{"name":"ui_type","description":"往窗口里的输入框输入文本（按控件 id 或标题关键词定位）。密码 / 验证码类控件不代填；往「发送 / 提交」类输入框打字同样要先经用户确认。输入完不会自动发出去——发送是另一次 ui_click。文本里不能带换行符：换行等于按回车键，目标应用会当场发出去。示例：先 activate_window 把目标窗口置前，否则报「目标窗口不在前台」；target 用 ui_inspect 返回的控件 id（形如 12358:0.7）；控件不支持按压时报「无法聚焦该控件」，改用 ui_click 先点它聚焦","parameters":{"type":"object","properties":{
    "windowId":{"type":"string","description":"窗口 id（list_windows 返回）"},
    "target":{"type":"string","description":"控件 id（ui_inspect 返回）或控件标题关键词"},
    "text":{"type":"string","description":"要输入的文本"}
  },"required":["windowId","target","text"]}}}"##;
pub const SCHEMA_WAIT_FOR_ELEMENT: &str = r##"{"type":"function","function":{"name":"wait_for_element","description":"等待窗口里出现某个控件（按标题 / 角色关键词），等到就返回它的 id，超时没出现就如实说明。用于「等对方发来消息再回复」这类时序","parameters":{"type":"object","properties":{
    "windowId":{"type":"string","description":"窗口 id（list_windows 返回）"},
    "query":{"type":"string","description":"要找的控件标题 / 角色关键词"},
    "timeoutMs":{"type":"integer","description":"可选：等待上限毫秒，默认 5000，最大 30000"}
  },"required":["windowId","query"]}}}"##;
pub const SCHEMA_WEB_SEARCH: &str = r##"{"type":"function","function":{"name":"web_search","description":"搜索互联网获取最新信息（配置 Tavily 或 Brave key 时走对应 API、双开报错，否则 Bing+百度网页抓取；返回标题/链接/摘要；润色/翻译/改写用户已给的文本不需要搜索）","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"搜索关键词"},
    "count":{"type":"integer","description":"结果条数，可选，1-10，默认 8"},
    "timeRange":{"type":"string","enum":["day","week","month","year"],"description":"时间范围，可选：day=24小时内 / week=一周内 / month=一月内 / year=一年内（Tavily/Brave 原生支持；Bing+百度抓取模式不支持并会明确提示）"},
    "site":{"type":"string","description":"限定站点域名，可选，如 github.com（不要带 https:// 前缀）"}
  },"required":["query"]}}}"##;
pub const SCHEMA_FETCH_URL: &str = r##"{"type":"function","function":{"name":"fetch_url","description":"抓取网页正文（仅 http/https 公网地址；返回纯文本，用于读链接/总结网页内容；长网页带「已截断」提示时用 offset 参数续读）","parameters":{"type":"object","properties":{
    "url":{"type":"string","description":"要抓取的网页地址；示例：返回 403/429 说明站点拦自动化，改抓该页的原始文件或 API 地址；Content-Type 不是网页文本（JSON / PDF）会被拒"},
    "offset":{"type":"integer","description":"字符偏移（可选，默认 0；上一页返回的「已截断」提示里给了 offset 值，用它续读）"}
  },"required":["url"]}}}"##;
pub const SCHEMA_GET_CURRENT_TIME: &str = r##"{"type":"function","function":{"name":"get_current_time","description":"获取当前日期时间和星期（涉及「今天/明天/昨天/周几/几点」类判断前必须先调，不要凭训练数据猜日期）","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_LOOKUP_RECEIPT: &str = r##"{"type":"function","function":{"name":"lookup_receipt","description":"调回历史工具调用的完整结果（工具消息不跨轮持久，上一轮的返回出了那一轮就不在上下文里了）。不传参数=列出本会话最近的工具调用凭证；传 tool_call_id / span_id / trace_id=调取那一条的完整返回。引用旧轮工具结果前必须先调本工具取回原文，不得凭记忆复述","parameters":{"type":"object","properties":{
    "tool_call_id":{"type":"string","description":"工具调用 id（assistant 消息里 tool_calls 里的 id），可选"},
    "span_id":{"type":"integer","description":"单条工具调用的 id（痕迹页逐条列出的那串数），可选"},
    "trace_id":{"type":"integer","description":"一次执行的痕迹 id（一次执行里的全部工具调用），可选"}
  },"required":[]}}}"##;
pub const SCHEMA_REMEMBER_FACT: &str = r##"{"type":"function","function":{"name":"remember_fact","description":"记住一条用户偏好/事实（跨会话长期记忆，重启不丢；key 简短规范名词 ≤50 字，value 内容 ≤500 字；同 key 覆盖更新；value 传空串删除该条；写入结果若提示相似已有记忆，优先用同 key 覆盖更新而非另开新 key 堆积）","parameters":{"type":"object","properties":{
    "key":{"type":"string","description":"简短规范名词，如「称呼」「偏好语言」「常用目录」"},
    "value":{"type":"string","description":"要记住的内容；空串 = 删除该条"},
    "category":{"type":"string","description":"分类（可选，默认 general）：profile 画像 / preference 偏好 / project 项目上下文 / general"},
    "importance":{"type":"integer","description":"重要度 1-5（可选，默认 3；用户明确要求长期遵守的给 4-5，琐碎信息 1-2）"},
    "source":{"type":"string","description":"来源（可选，默认 user_stated）：user_stated 用户明确说的 / model_inferred 模型推断的"}
  },"required":["key","value"]}}}"##;
pub const SCHEMA_RECALL_FACTS: &str = r##"{"type":"function","function":{"name":"recall_facts","description":"回忆长期记忆（相关记忆每轮已自动注入，一般无需调用；只在要浏览全部记忆或按关键词检索时才调）","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"可选；给了按相关度检索返回 top-5，不给则全量读回"}
  }}}}"##;
pub const SCHEMA_RECORD_LESSON: &str = r##"{"type":"function","function":{"name":"record_lesson","description":"记录一条经验教训（跨会话长期记忆；被用户纠正、工具调用连续失败、发现更优做法时调用；同类场景的教训会自动合并，不会堆积）","parameters":{"type":"object","properties":{
    "lesson":{"type":"string","description":"教训内容（≤800 字）：什么场景下应该/不应该怎么做，以及原因"},
    "scenario":{"type":"string","description":"场景标签（可选 ≤50 字），如工具名或任务类型：create_ppt、批量执行、文档修订"}
  },"required":["lesson"]}}}"##;
/// create_skill：机器人自建技能（SKILL.md + 可选脚本/模板附件）——文件长期保留，路由仅本会话生效
pub const SCHEMA_CREATE_SKILL: &str = r##"{"type":"function","function":{"name":"create_skill","description":"把本次对话中沉淀的可复用流程固化为技能（skill）文件；触发路由仅本会话生效（重启后不再自动触发，用户可在设置页把该技能提升为全局）。适合：用户明确说「把这个流程存成技能/记住这个做法」，或同一多步流程第二次被手动描述时。content 为完整 SKILL.md 文本（YAML frontmatter 必含 name 与 description；可声明 risk_level/mode/max_steps/intents 触发关键词；mode 仅限 auto|interactive，省略时低风险默认 auto——auto 命中后不经 LLM 直接执行「## Step N:」加工具调用行形式的步骤块，纯散文描述的流程必须显式写 mode: interactive；其余步骤用 ## 标题分节）。创建后本会话立即生效：本会话内用户说出 intents 关键词即进入该技能。同名技能已存在、或 description/intents 命中危险关键词会被拒绝","parameters":{"type":"object","properties":{"name":{"type":"string","description":"技能名：仅字母/数字/-/_"},"content":{"type":"string","description":"完整 SKILL.md 文本（frontmatter + 正文步骤）"},"files":{"type":"array","description":"可选附件（脚本/模板），路径相对技能目录","items":{"type":"object","properties":{"path":{"type":"string","description":"相对路径，如 scripts/run.py"},"content":{"type":"string","description":"文件文本内容"}}}}},"required":["name","content"]}}}"##;

/// install_skill：从远程源安装技能（GitHub 仓库 / 腾讯 SkillHub / zip 直链），装上即生效
pub const SCHEMA_INSTALL_SKILL: &str = r##"{"type":"function","function":{"name":"install_skill","description":"从远程源把技能（skill）装到本机技能目录，装上即生效并进入「已安装技能」清单。用户说「安装/装一下 XX 技能」「把这个 GitHub 仓库装成技能」「从 SkillHub 装 XX」时用它。source 三种写法：① GitHub 仓库地址（https://github.com/owner/repo；仓库里含多个技能时用 skill 参数指定技能目录名，或 source 直接写 owner/repo#技能名）② 腾讯 SkillHub 技能页地址（https://skillhub.cn/skills/xxx）③ zip 直链。安装会弹确认窗等用户点头；技能名不合法（如中文）时自动改用来源里的 slug 作技能名。装完后续用 use_skill 读取并执行","parameters":{"type":"object","properties":{
"source":{"type":"string","description":"技能来源：GitHub 仓库地址 / SkillHub 技能页地址 / zip 直链 / owner/repo#技能名"},
"skill":{"type":"string","description":"可选：仓库内的技能目录名——仓库含多个技能时必填（如 skill-hub-query）"}
},"required":["source"]}}}"##;

/// search_skills：在腾讯 SkillHub 上搜可安装的技能（公开 API，免鉴权）
pub const SCHEMA_SEARCH_SKILLS: &str = r##"{"type":"function","function":{"name":"search_skills","description":"在腾讯 SkillHub（中文技能商店）搜索可安装的技能。用户说「找个能做 XX 的技能」「SkillHub 上有没有 XX」「装个 XX 技能」且还不知道具体技能名时先搜它。返回候选的名称、简介、slug、热度，以及可直接交给 install_skill 的安装地址。搜完先把候选列给用户挑，用户选定再调 install_skill 装","parameters":{"type":"object","properties":{
"keyword":{"type":"string","description":"搜索关键词（中文或英文，如「周报」「PDF 处理」「前端」）"},
"limit":{"type":"integer","description":"返回条数，可选，默认 8，最多 20"}
},"required":["keyword"]}}}"##;

pub const SCHEMA_USE_SKILL: &str = r##"{"type":"function","function":{"name":"use_skill","description":"读取已安装技能（skill）的完整文档并按文档步骤执行。任务涉及的每个相关技能都要读（可多次调用）：例如做 PPT 时，若清单里同时有编排、生成、配色、风格类技能，应逐个读取、取长补短综合运用，不要只读一个","parameters":{"type":"object","properties":{
    "name":{"type":"string","description":"技能名（系统提示词「已安装技能」清单里的名称，一次一个，可多次调用）"},
    "params":{"type":"object","description":"技能参数（可选；键=参数名，值=字符串）。技能声明了必填参数时必须提供（缺失会拒绝启动并列出缺什么），声明了默认值的参数可省略","additionalProperties":{"type":"string"}}
  },"required":["name"]}}}"##;
// 程序 M2（聊天即 IDE）：apps_list 防重名 / apps_create 落草稿 / apps_update_files 整文件集修改
pub const SCHEMA_APPS_LIST: &str = r##"{"type":"function","function":{"name":"apps_list","description":"列出当前所有程序（草稿+已启用，不含已归档），每条含 id/名称/状态/一句话说明。创建新程序前必调：防 id 重名、避免做出功能重复的程序","parameters":{"type":"object","properties":{}}}}"##;
pub const SCHEMA_APPS_CREATE: &str = r####"{"type":"function","function":{"name":"apps_create","description":"把写好的确定性 Python 小程序登记为「程序」（草稿态）：脚本与附加文件落盘 data/apps/<id>/，用户在程序页试跑，满意后再上架。适合每月重复的固定流程（工资单匹配、名册查询等）。创建前先 apps_list 防重名；规格含糊先用 ask_user 问清、用 read_text_file 看用户给的真实样例。脚本契约（必须遵守）：python3 零依赖为佳；开头是校验段——检查 input/ 文件存在、期望列名齐全，对不上向 stderr 打「❌+中文原因」并 sys.exit(2)，宁可响亮失败不许静默空跑；用 sys.argv[1]（app 目录绝对路径）解析 data/ input/，产物写 argv[2]（本次运行产物目录 output/<run_id>/，运行器已建好），不依赖 cwd；最后一行必须是 print(\"##RESULT## \" + json.dumps({\"summary\":\"人话总结\"}, ensure_ascii=False))，哨兵行前必须有换行，summary 控制在 300 字内；禁联网；敏感数据（身份证号等）放 data/ 配置文件，不硬编码在脚本里","parameters":{"type":"object","properties":{"id":{"type":"string","description":"程序 id=目录名：仅字母/数字/-/_，不能含 / 或 .."},"name":{"type":"string","description":"程序页卡片标题，≤200 字"},"description":{"type":"string","description":"一句话说明（卡片副标题），≤500 字"},"icon":{"type":"string","description":"卡片 emoji 图标，1-4 字符，缺省 📦"},"input_schema":{"description":"输入声明：{\"kind\":\"folder\"}（文件夹模式，推荐）或 {\"kind\":\"form\",\"fields\":[{\"key\",\"label\",\"type\":\"text|number|date\",\"required\",\"placeholder\"}]}；缺省=文件夹模式"},"script":{"type":"string","description":"完整 app.py 源码全文（不许片段或 diff）"},"files":{"type":"array","description":"可选附加文件，只允许 data/ 之下（如姓名-身份证对照表 csv）","items":{"type":"object","properties":{"path":{"type":"string","description":"相对路径，如 data/roster.csv"},"content":{"type":"string","description":"文件全文"}}}},"manual":{"type":"string","description":"给用户看的使用说明：这个程序做什么、要放什么文件、点哪里、出什么，≤8000 字"},"timeoutSecs":{"type":"integer","description":"声明这个程序单次运行需要多久（秒，可选）：耗时随输入规模增长的活（转写长录音、装依赖、批处理）必须在这里声明，否则运行按内置默认 60 秒会被中途掐掉；上限 1800"}},"required":["id","name","script"]}}}"####;
pub const SCHEMA_APPS_UPDATE_FILES: &str = r####"{"type":"function","function":{"name":"apps_update_files","description":"修改已有程序（整文件集提交）：改脚本必须传完整新 script 全文（不做增量合并、不收 diff）；也可改输入声明/名称/说明/图标/说明书/单次运行时长。status=\"enabled\" 表示上架启用（务必等用户在聊天里明确满意后再传），status=\"draft\" 回草稿；归档是用户在程序页的主动操作，工具不受理。脚本契约同 apps_create（开头校验段 + ##RESULT## 哨兵收尾 + argv[1] 解析目录 + 产物写 argv[2] + 禁联网）。程序不存在或已归档会被拒绝","parameters":{"type":"object","properties":{"id":{"type":"string","description":"要修改的程序 id（先 apps_list 查）"},"script":{"type":"string","description":"完整 app.py 源码全文；不传=脚本不动"},"files":{"type":"array","description":"附加文件（只允许 data/ 之下），同名整体覆盖","items":{"type":"object","properties":{"path":{"type":"string","description":"相对路径，如 data/roster.csv"},"content":{"type":"string","description":"文件全文"}}}},"name":{"type":"string","description":"新标题；不传=保持不变"},"description":{"type":"string","description":"新说明；不传=保持不变"},"icon":{"type":"string","description":"新 emoji 图标；不传=保持不变"},"input_schema":{"description":"输入声明（对象或 JSON 串）；不传=保持不变"},"manual":{"type":"string","description":"新使用说明；不传=保持不变"},"timeoutSecs":{"type":"integer","description":"改声明的单次运行时长（秒）：不传=保持不变，传 0=取消声明（回落内置默认 60 秒），传 N=声明 N 秒（上限 1800）"},"status":{"type":"string","description":"draft 或 enabled（enabled=上架启用，需用户先明确满意）"}},"required":["id"]}}}"####;
// 工作流执行提问（主可见）
pub const SCHEMA_ASK_USER: &str = r##"{"type":"function","function":{"name":"ask_user","description":"向用户提一个问题并等待回答（仅任务执行中可用：工作流节点与手动执行的任务卡；聊天中不要用本工具，直接文字提问即可。提问会进通知中心，用户可能几小时后才回答）。必须携带 assumption=你的推荐假设：用户不回答时按假设继续，给不出假设的问题不许问。仅当缺关键信息且无法从任务卡/上游产出/附件推断时才用；预算为全局设置（1–5，默认 3，工作流与任务卡共用），超预算直接按假设继续","parameters":{"type":"object","properties":{
    "question":{"type":"string","description":"要问的问题，≤200 字，具体明确"},
    "why":{"type":"string","description":"一句话说明为什么要问，可选"},
    "options":{"type":"array","items":{"type":"string"},"description":"候选项（可选，≤4 个、每个 ≤40 字），用户可点选也可自由回答"},
    "assumption":{"type":"string","description":"你的推荐假设，必填——用户未回答时按它继续执行"}
  },"required":["question","assumption"]}}}"##;
// 子 agent 编排三工具（主 agent 可见）
pub const SCHEMA_SPAWN_SUBAGENT: &str = r##"{"type":"function","function":{"name":"spawn_subagent","description":"派发受管子 agent 执行单一目标长任务（非阻塞，立即返回 subagentId/status）。适用：预计超 5 轮工具调用、多来源调研、写代码跑脚本、用户要求后台/并行。objective 单一目标；acceptanceCriteria 必填且每条可检验（不要写「调研清楚」，要写「覆盖至少 5 个产品，每个含官网 URL，输出 report.md」）；contextSummary 只给必要背景，不要倒主对话全文。完成后用 check_subagent 轮询结果再汇总。同一会话里重复派发完全相同的目标（档位/目标/验收标准全同）不会建第二条活行：返回同一个 subagentId，继续 check 它即可","parameters":{"type":"object","properties":{
    "objective":{"type":"string","description":"单一目标（一句话说清做什么）"},
    "onion":{"type":"string","description":"per-onion 配置档名（可选）：probe=只读探查产出素材 / writereditor=撰写与写代码 / validator=只读验收。指定后本子 agent 的工具会被该配置收窄、角色与轮数上限一并生效，且服务端会拒绝配置清单外的工具——这是控制子 agent 权限的唯一入口。三段式流水线（探查→撰写→验收）就靠它分别派发"},
    "profile":{"type":"string","enum":["research","coder","general"],"description":"工具档位：research=联网调研写报告 / coder=读代码跑 Python 写文件 / general=两者并集（与 stage 同给时白名单档优先，stage 只换角色语义层）。不给时按 onion 配置的 profile 派生；两者都不给则按 stage 派生（recon/verifier→research，executor→coder），都没有则报错"},
    "stage":{"type":"string","enum":["recon","executor","verifier"],"description":"三段式派发阶段（可选）：recon=只读调研 / executor=基于报告动手实现 / verifier=逐条验收。不给 stage 时仅按 profile 走基线提示词；只给 stage 不给 profile 时按默认派生（recon/verifier→research，executor→coder）"},
    "acceptanceCriteria":{"type":"array","items":{"type":"string"},"minItems":1,"description":"验收标准，每条可检验"},
    "contextSummary":{"type":"string","description":"必要上下文摘要（可选；不含主对话全文）"},
    "budget":{"type":"object","properties":{"maxTurns":{"type":"integer","description":"工具循环轮数上限（钳到硬顶）"},"maxToolCalls":{"type":"integer","description":"累计工具调用上限；整个 budget 不给时取全局熔断配置（设置页参数 loop.max_function_calls），此处显式给值才覆盖"}},"description":"预算两硬顶（可选；不给则按默认与全局配置取值）"},
    "modelProfile":{"type":"string","description":"指定模型名（可选；未指定/无效回退当前激活模型）"},
    "parentTaskId":{"type":"string","description":"主卡 id（可选；编排计划卡，缺卡报错）"}
  },"required":["objective","acceptanceCriteria"]}}}"##;
pub const SCHEMA_CHECK_SUBAGENT: &str = r##"{"type":"function","function":{"name":"check_subagent","description":"查询子 agent 状态（幂等可轮询）：status=queued|running|succeeded|failed|cancelled|budget_exceeded + 进度 + 收尾结果（summary/artifacts/blockers）。waitMs 是可放心给大的等待预算（上限 600000，即 10 分钟）：服务端内部递进等待，子任务一转入终态立刻返回，不必自己反复短轮询","parameters":{"type":"object","properties":{
    "subagentId":{"type":"string","description":"子 agent id（spawn 返回值）"},
    "waitMs":{"type":"integer","description":"等待预算毫秒数（可选，0=立即返回，上限 600000）。一次给足比反复小值轮询省轮次：每查一次你都要重新想一轮"}
  },"required":[]}}}"##;
pub const SCHEMA_CANCEL_SUBAGENT: &str = r##"{"type":"function","function":{"name":"cancel_subagent","description":"取消子 agent：置 cancelled 并停止其执行；已终态则 no-op。用户说「停掉/不用继续了」且能对应到在途子 agent 时用","parameters":{"type":"object","properties":{
    "subagentId":{"type":"string","description":"子 agent id（spawn 返回值）"},
    "reason":{"type":"string","description":"取消原因（可选）"}
  },"required":[]}}}"##;
// 子 agent 白名单内工具（不进主 agent schema）
pub const SCHEMA_WRITE_ARTIFACT_FILE: &str = r##"{"type":"function","function":{"name":"write_artifact_file","description":"把文本内容写进自己的产物目录（gen_dir/subagents/{subagentId}/，报告/中间结果落地用）。filename 为纯文件名（不含路径分隔符），已存在会自动加 (n) 序号不覆盖","parameters":{"type":"object","properties":{
    "filename":{"type":"string","description":"纯文件名（不含路径分隔符 / 与 \\、不允许 .. 路径段与 NUL；Windows 保留设备名如 CON/NUL 拒绝），如 report.md / data.csv；同名自动加 (n) 序号"},
    "content":{"type":"string","description":"完整文本内容"}
  },"required":["filename","content"]}}}"##;
pub const SCHEMA_READ_OWN_CARD: &str = r##"{"type":"function","function":{"name":"read_own_card","description":"重读自己的执行契约（objective / acceptanceCriteria / contextSummary / budgetUsage 实时余量 / status）：每轮开始建议先调，契约或余量有变化则调整计划；status=cancelled = 派发已被取消，立即停止新探索并收尾","parameters":{"type":"object","properties":{}}}}"##;
// 电脑辅助 Tier1（只「看」与「打开」，无鼠标键盘）
pub const SCHEMA_REVEAL_PATH: &str = r##"{"type":"function","function":{"name":"reveal_path","description":"在访达（macOS）/资源管理器（Windows）中定位显示文件或文件夹（只定位，不打开文件本身；仅限白名单目录内路径，与读文件同一权限闸）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件或目录绝对路径（支持 ~ 开头）"}
  },"required":["path"]}}}"##;
pub const SCHEMA_OPEN_URL: &str = r##"{"type":"function","function":{"name":"open_url","description":"用系统默认浏览器打开网页（仅 http/https 公网地址；本机/内网地址会被拒绝）","parameters":{"type":"object","properties":{
    "url":{"type":"string","description":"要打开的网页地址"}
  },"required":["url"]}}}"##;
pub const SCHEMA_CLIPBOARD_WRITE: &str = r##"{"type":"function","function":{"name":"clipboard_write","description":"把文本写入系统剪贴板，用户可直接粘贴（会覆盖剪贴板原内容；超长内容请分段）","parameters":{"type":"object","properties":{
    "text":{"type":"string","description":"要复制的文本"}
  },"required":["text"]}}}"##;
pub const SCHEMA_SCREENSHOT: &str = r##"{"type":"function","function":{"name":"screenshot","description":"截取主显示器画面。截图会直接作为图片附在工具结果之后，用你的视觉能力读取内容（PNG 同时落 AI_Gen_Files 留档；需要系统屏幕录制权限，macOS 未授权时会得到壁纸/黑图）","parameters":{"type":"object","properties":{}}}}"##;
// 文件编辑（写白名单 ≠ 读白名单，白名单外逐次确认）
pub const SCHEMA_EDIT_FILE: &str = r##"{"type":"function","function":{"name":"edit_file","description":"对文本文件做字符串替换（默认 literal 精确模式；可传 mode=regex 启用正则全部匹配替换；详见参数说明）。literal 模式小步修改首选（先 read_text_file 确认原文再改）：oldString 必须与文件内容一致且唯一（多处命中报错；行尾空白/CRLF 差异自动容错）。改完建议 read_text_file 复核","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径（仅限可写目录：AI_Gen_Files + 任务卡绑定文件夹 + 设置页 allowedDirs；白名单外会弹确认）"},
    "oldString":{"type":"string","description":"要替换的原文：literal 模式=精确匹配（须唯一；含足够上下文）；regex 模式=正则模式串（全部匹配替换）"},
    "newString":{"type":"string","description":"替换后的新文本：literal 模式=可为空串=删除该段；regex 模式=可用 $1/$2 命名/编号捕获组引用，replace 语义与 sed g 一致"},
    "mode":{"type":"string","enum":["literal","regex"],"default":"literal","description":"匹配模式：literal=精确（默认；oldString 须唯一；多处命中报错）；regex=正则（oldString 当作 Rust regex 模式串，newString 支持 $1/$2 捕获组，全部命中替换；非法 pattern 报错；空替换会把文件清空时拒绝并改用 write_file）"}
  },"required":["path","oldString","newString"]}}}"##;
pub const SCHEMA_WRITE_FILE: &str = r##"{"type":"function","function":{"name":"write_file","description":"创建新文件或整体写入内容（仅限可写目录：AI_Gen_Files + 任务卡绑定文件夹 + 设置页 allowedDirs；覆盖已存在文件需要用户确认）。修改已有文件优先用 edit_file（精确替换更安全）。是否生成成功以本工具返回为准：返回 Err 就是没生成，不得向用户声称已生成；成功时必须把返回的完整路径告知用户","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"目标文件绝对路径（父目录必须已存在）"},
    "content":{"type":"string","description":"完整文件内容（UTF-8 文本；不能含 NUL 字节；上限 2MB）"}
  },"required":["path","content"]}}}"##;
pub const SCHEMA_UPDATE_PLAN: &str = r##"{"type":"function","function":{"name":"update_plan","description":"更新本会话的执行计划（整表替换：每次调用都要交出完整清单，不要只交增量）。用途：多步任务开工前先列出步骤；每完成一步立刻调用一次更新该步状态，不要攒到最后一起补。状态三态：pending 未开始 / in_progress 正在做 / completed 已完成，任一时刻至多一条 in_progress。不要为了好看而虚报完成：completed 只给真正做完的步骤。什么时候建计划（命中任意一条就建）：要产出多份文件或多个交付物、任务要分阶段推进、用户给了多份材料要合并整合、技能文档里有多个阶段——读完技能文档、看清它有哪几个阶段之后再建也算来得及。什么时候不要用：单一目标的小任务、琐碎操作、纯信息问答——这类直接做，别建计划","parameters":{"type":"object","properties":{
    "explanation":{"type":"string","description":"相对上一版改了什么（可选）：新增/删除/调整了哪些步骤、为什么改；只写变更说明，不要把清单再抄一遍"},
    "plan":{"type":"array","description":"完整步骤清单（整表替换：调用一次即覆盖上一次的全部内容）","items":{"type":"object","properties":{
      "step":{"type":"string","description":"这一步要做什么（祈使句，一句话，别写成一整段）"},
      "status":{"type":"string","enum":["pending","in_progress","completed"],"description":"该步状态：pending 未开始 / in_progress 正在做 / completed 已完成"}
    },"required":["step","status"]}}
  },"required":["plan"]}}}"##;
use std::collections::HashMap;

/// TOOLS_TABLE 的 O(1) HashMap 索引。
/// dispatch 的 O(n) `TOOLS_TABLE.iter().find(...)` 改为 O(1) `tools_index().get(name)`。
/// OnceLock 保证只有一次初始化；索引里全部为 `&'static ToolDef` 引用，零额外分配（除 HashMap 表本身）。
static TOOLS_INDEX: OnceLock<HashMap<&'static str, &'static ToolDef>> = OnceLock::new();

pub fn tools_index() -> &'static HashMap<&'static str, &'static ToolDef> {
    TOOLS_INDEX.get_or_init(|| TOOLS_TABLE.iter().map(|t| (t.name, t)).collect())
}

/// 内置工具总数（= TOOLS_TABLE 条目数 = SCHEMA_* 常量数）。
/// 注释与文档一律引用本常量而不另写数字；增删工具时只改这里一处，
/// `builtin_tool_count_matches_table` 单测负责在数目对不上时红灯。
pub const BUILTIN_TOOL_COUNT: usize = 65;

// TOOLS_TABLE（内置工具单源真相：BUILTIN_TOOL_COUNT 个，其中 SUBAGENT_ONLY_TOOLS
// 那几个仅子 agent 可见，主 agent 少这么多）
pub static TOOLS_TABLE: &[ToolDef] = &[
    ToolDef {
        name: "query_tasks",
        schema: SCHEMA_QUERY_TASKS,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_query_tasks,
    },
    ToolDef {
        name: "query_single_task",
        schema: SCHEMA_QUERY_SINGLE_TASK,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_query_single_task,
    },
    ToolDef {
        name: "create_task",
        schema: SCHEMA_CREATE_TASK,
        mutating: true,
        claims_patterns: &["已创建", "已新建", "新建了", "创建了", "已新增"],
        max_output_bytes: 8192,
        call: call_create_task,
    },
    ToolDef {
        name: "complete_task",
        schema: SCHEMA_COMPLETE_TASK,
        mutating: true,
        claims_patterns: &["已标记为完成", "标记为完成", "已完成任务", "已完成「"],
        max_output_bytes: 8192,
        call: call_complete_task,
    },
    ToolDef {
        name: "delete_task",
        schema: SCHEMA_DELETE_TASK,
        mutating: true,
        claims_patterns: &[
            "已删除",
            "已经删除",
            "已彻底删除",
            "已经彻底删除",
            "已搬移",
            "已搬到回收站",
            "移至回收站",
            "已移除",
            "已扔掉",
        ],
        max_output_bytes: 8192,
        call: call_delete_task,
    },
    ToolDef {
        name: "edit_task",
        schema: SCHEMA_EDIT_TASK,
        mutating: true,
        claims_patterns: &["已修改", "已更新", "已改成", "已改", "已调整", "已编辑"],
        max_output_bytes: 8192,
        call: call_edit_task,
    },
    ToolDef {
        name: "add_subtask",
        schema: SCHEMA_ADD_SUBTASK,
        mutating: true,
        claims_patterns: &[
            "已添加子任务",
            "添加子任务",
            "已加上子任务",
            "已新增子任务",
            "已加子任务",
        ],
        max_output_bytes: 8192,
        call: call_add_subtask,
    },
    ToolDef {
        name: "toggle_subtask",
        schema: SCHEMA_TOGGLE_SUBTASK,
        mutating: true,
        claims_patterns: &["已勾选", "勾选了", "已标记子任务完成", "子任务已勾选"],
        max_output_bytes: 8192,
        call: call_toggle_subtask,
    },
    ToolDef {
        name: "remove_subtask",
        schema: SCHEMA_REMOVE_SUBTASK,
        mutating: true,
        claims_patterns: &[
            "已删除子任务",
            "已移除子任务",
            "子任务已删除",
            "子任务已移除",
            "已清掉子任务",
        ],
        max_output_bytes: 8192,
        call: call_remove_subtask,
    },
    ToolDef {
        name: "read_text_file",
        schema: SCHEMA_READ_TEXT_FILE,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 65536,
        call: call_read_text_file,
    },
    ToolDef {
        name: "ocr_image",
        schema: SCHEMA_OCR_IMAGE,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_ocr_image,
    },
    ToolDef {
        name: "grep_files",
        schema: SCHEMA_GREP_FILES,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_grep_files,
    },
    ToolDef {
        name: "list_files",
        schema: SCHEMA_LIST_FILES,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 65536,
        call: call_list_files,
    },
    ToolDef {
        name: "link_file_to_task",
        schema: SCHEMA_LINK_FILE_TO_TASK,
        mutating: true,
        claims_patterns: &[
            "已绑定",
            "已关联",
            "绑定到任务",
            "已绑定文件",
            "已清空",
            "已清空文件",
            "已解绑",
        ],
        max_output_bytes: 8192,
        call: call_link_file_to_task,
    },
    ToolDef {
        name: "extract_document",
        schema: SCHEMA_EXTRACT_DOCUMENT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_extract_document,
    },
    ToolDef {
        name: "create_word",
        schema: SCHEMA_CREATE_WORD,
        mutating: true,
        claims_patterns: &[
            "已生成 Word",
            "已生成 word",
            "已创建 Word",
            "Word 文件已生成",
            "已写入 Word",
        ],
        max_output_bytes: 8192,
        call: call_create_word,
    },
    ToolDef {
        name: "create_word_revisions",
        schema: SCHEMA_CREATE_WORD_REVISIONS,
        mutating: true,
        claims_patterns: &["已修订 Word", "已应用修订", "已写入修订", "修订已应用"],
        max_output_bytes: 8192,
        call: call_create_word_revisions,
    },
    ToolDef {
        name: "create_excel",
        schema: SCHEMA_CREATE_EXCEL,
        mutating: true,
        claims_patterns: &[
            "已生成 Excel",
            "已生成 excel",
            "已创建 Excel",
            "Excel 文件已生成",
            "已写入 Excel",
        ],
        max_output_bytes: 8192,
        call: call_create_excel,
    },
    ToolDef {
        name: "create_ppt",
        schema: SCHEMA_CREATE_PPT,
        mutating: true,
        claims_patterns: &[
            "已生成 PPT",
            "已生成 ppt",
            "已生成幻灯片",
            "PPT 文件已生成",
            "已写入 PPT",
        ],
        max_output_bytes: 8192,
        call: call_create_ppt,
    },
    ToolDef {
        name: "create_pdf",
        schema: SCHEMA_CREATE_PDF,
        mutating: true,
        claims_patterns: &[
            "已生成 PDF",
            "已生成 pdf",
            "已创建 PDF",
            "PDF 文件已生成",
            "已写入 PDF",
        ],
        max_output_bytes: 8192,
        call: call_create_pdf,
    },
    ToolDef {
        name: "run_python",
        schema: SCHEMA_RUN_PYTHON,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_run_python,
    },
    ToolDef {
        name: "web_search",
        schema: SCHEMA_WEB_SEARCH,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_web_search,
    },
    ToolDef {
        name: "fetch_url",
        schema: SCHEMA_FETCH_URL,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 65536,
        call: call_fetch_url,
    },
    ToolDef {
        name: "get_current_time",
        schema: SCHEMA_GET_CURRENT_TIME,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_get_current_time,
    },
    ToolDef {
        name: "lookup_receipt",
        schema: SCHEMA_LOOKUP_RECEIPT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_lookup_receipt,
    },
    ToolDef {
        name: "remember_fact",
        schema: SCHEMA_REMEMBER_FACT,
        mutating: true,
        claims_patterns: &[
            "已记住",
            "已记下",
            "已记录偏好",
            "偏好已记",
            "已记住你的偏好",
        ],
        max_output_bytes: 8192,
        call: call_remember_fact,
    },
    ToolDef {
        name: "recall_facts",
        schema: SCHEMA_RECALL_FACTS,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_recall_facts,
    },
    ToolDef {
        name: "record_lesson",
        schema: SCHEMA_RECORD_LESSON,
        mutating: true,
        claims_patterns: &[
            "已保存",
            "已保存到 AI_Gen_Files",
            "已记住你的偏好",
            "已记下教训",
            "已存档",
        ],
        max_output_bytes: 8192,
        call: call_record_lesson,
    },
    ToolDef {
        name: "use_skill",
        schema: SCHEMA_USE_SKILL,
        mutating: false,
        claims_patterns: &[],
        // 文档类输出：技能正文是模型要照着执行的流程说明，截掉中段等于删任务
        // （实测 57KB 正文按 8KB 头尾节选后，「工作流程（七步法）」整节丢失）。
        // 上限给到 65536，装得下任何一篇技能正文（正文自身另有 50KB 上限）。
        max_output_bytes: 65536,
        call: call_use_skill,
    },
    ToolDef {
        name: "create_skill",
        schema: SCHEMA_CREATE_SKILL,
        mutating: true,
        claims_patterns: &["已创建技能", "已新建技能", "已注册技能"],
        max_output_bytes: 8192,
        call: call_create_skill,
    },
    ToolDef {
        name: "install_skill",
        schema: SCHEMA_INSTALL_SKILL,
        mutating: true,
        claims_patterns: &["已安装技能", "技能已安装"],
        max_output_bytes: 8192,
        call: call_install_skill,
    },
    ToolDef {
        name: "search_skills",
        schema: SCHEMA_SEARCH_SKILLS,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 16384,
        call: call_search_skills,
    },
    // 子 agent 编排（主 agent 可见，追加表尾）
    ToolDef {
        name: "spawn_subagent",
        schema: SCHEMA_SPAWN_SUBAGENT,
        mutating: true,
        claims_patterns: &[
            "已派发",
            "已派出",
            "已派子任务",
            "已启动子任务",
            "已创建子任务",
        ],
        max_output_bytes: 8192,
        call: call_spawn_subagent,
    },
    ToolDef {
        name: "check_subagent",
        schema: SCHEMA_CHECK_SUBAGENT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_check_subagent,
    },
    ToolDef {
        name: "cancel_subagent",
        schema: SCHEMA_CANCEL_SUBAGENT,
        mutating: true,
        claims_patterns: &["已取消子任务", "已取消派发", "已终止子任务"],
        max_output_bytes: 8192,
        call: call_cancel_subagent,
    },
    // 电脑辅助 Tier1（只「看」与「打开」）
    ToolDef {
        name: "reveal_path",
        schema: SCHEMA_REVEAL_PATH,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_reveal_path,
    },
    ToolDef {
        name: "open_url",
        schema: SCHEMA_OPEN_URL,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_open_url,
    },
    ToolDef {
        name: "clipboard_write",
        schema: SCHEMA_CLIPBOARD_WRITE,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_clipboard_write,
    },
    ToolDef {
        name: "screenshot",
        schema: SCHEMA_SCREENSHOT,
        // 截图真写盘（PNG 落 AI_Gen_Files）——按变更类登记，否则「已截屏 /
        // 已保存截图」这类声称对幻觉守卫完全不可见（模型不调工具也能报）。
        mutating: true,
        claims_patterns: &["已截屏", "已保存截图", "截图已保存"],
        max_output_bytes: 8192,
        call: call_screenshot,
    },
    // 文件编辑（写白名单 ≠ 读白名单）
    ToolDef {
        name: "edit_file",
        schema: SCHEMA_EDIT_FILE,
        mutating: true,
        claims_patterns: &["已修改", "已编辑"],
        max_output_bytes: 8192,
        call: call_edit_file,
    },
    ToolDef {
        name: "write_file",
        schema: SCHEMA_WRITE_FILE,
        mutating: true,
        claims_patterns: &["已写入", "已创建文件"],
        max_output_bytes: 8192,
        call: call_write_file,
    },
    // 程序 M2（聊天即 IDE）：apps_list 防重名 / apps_create 落草稿 / apps_update_files 整文件集修改
    ToolDef {
        name: "apps_list",
        schema: SCHEMA_APPS_LIST,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_apps_list,
    },
    ToolDef {
        name: "apps_create",
        schema: SCHEMA_APPS_CREATE,
        mutating: true,
        claims_patterns: &["已创建程序", "已新建程序", "已登记程序", "程序已创建"],
        max_output_bytes: 8192,
        call: call_apps_create,
    },
    ToolDef {
        name: "apps_update_files",
        schema: SCHEMA_APPS_UPDATE_FILES,
        mutating: true,
        claims_patterns: &["已更新程序", "已修改程序", "程序已更新", "已上架"],
        max_output_bytes: 8192,
        call: call_apps_update_files,
    },
    // 子 agent 白名单工具（不进主 agent 默认 schema）
    ToolDef {
        name: "write_artifact_file",
        schema: SCHEMA_WRITE_ARTIFACT_FILE,
        mutating: true,
        claims_patterns: &["已写入产物", "产物已写入", "已保存产物", "已写入报告"],
        max_output_bytes: 8192,
        call: call_write_artifact_file,
    },
    ToolDef {
        name: "read_own_card",
        schema: SCHEMA_READ_OWN_CARD,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_read_own_card,
    },
    ToolDef {
        name: "ask_user",
        schema: SCHEMA_ASK_USER,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_ask_user,
    },
    // Python AST 探查。追加在表尾：tools_json_matches_baseline 只比前 28 项
    // 前缀，插在中间会无谓地改动基线前缀（基线是「核心工具集」的快照，不是
    // 全量清单的镜像）。顺序只影响主 agent 看到工具的排列，无语义。
    ToolDef {
        name: "ast_inspect",
        schema: SCHEMA_AST_INSPECT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_ast_inspect,
    },
    // 归档工具。追加在表尾：tools_json_matches_baseline 只比前 28 项前缀，
    // 插在中间会无谓地改动基线前缀（同 ast_inspect 的理由）。
    ToolDef {
        name: "unpack_archive",
        schema: SCHEMA_UNPACK_ARCHIVE,
        mutating: true,
        claims_patterns: &["已解压", "已解包", "已解压出"],
        max_output_bytes: 8192,
        call: call_unpack_archive,
    },
    ToolDef {
        name: "pack_archive",
        schema: SCHEMA_PACK_ARCHIVE,
        mutating: true,
        claims_patterns: &["已打包", "已压缩", "已生成压缩包"],
        max_output_bytes: 8192,
        call: call_pack_archive,
    },
    // 文件复制 / 移动 / 删除。追加在表尾：与归档工具同理由——基线只比前缀。
    ToolDef {
        name: "manage_file",
        schema: SCHEMA_MANAGE_FILE,
        mutating: true,
        claims_patterns: &["已复制", "已拷贝", "已移动", "已移到"],
        max_output_bytes: 8192,
        call: call_manage_file,
    },
    // 文件进系统剪贴板。追加在表尾：同 manage_file 的理由（基线只比前缀）。
    // 非 mutating：剪贴板不是文件/任务卡的变更，与 clipboard_write 同档。
    ToolDef {
        name: "copy_file_to_clipboard",
        schema: SCHEMA_COPY_FILE_TO_CLIPBOARD,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_copy_file_to_clipboard,
    },
    // 提醒三件套。追加在表尾：同 manage_file 的理由（基线只比前缀）。
    ToolDef {
        name: "schedule_reminder",
        schema: SCHEMA_SCHEDULE_REMINDER,
        mutating: true,
        claims_patterns: &["已设提醒", "已设置提醒", "提醒已设置", "已添加提醒"],
        max_output_bytes: 8192,
        call: call_schedule_reminder,
    },
    ToolDef {
        name: "list_reminders",
        schema: SCHEMA_LIST_REMINDERS,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_list_reminders,
    },
    ToolDef {
        name: "cancel_reminder",
        schema: SCHEMA_CANCEL_REMINDER,
        mutating: true,
        claims_patterns: &["已取消提醒", "提醒已取消"],
        max_output_bytes: 8192,
        call: call_cancel_reminder,
    },
    // UI 自动化六件套。追加在表尾：同 manage_file 的理由（基线只比前缀）。
    // 点击 / 输入是 mutating：它们能在别的应用里产生真实副作用（消息真发出去了），
    // 声称「已点击 / 已输入」必须能对上凭证。
    ToolDef {
        name: "list_windows",
        schema: SCHEMA_LIST_WINDOWS,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_list_windows,
    },
    ToolDef {
        name: "activate_window",
        schema: SCHEMA_ACTIVATE_WINDOW,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_activate_window,
    },
    ToolDef {
        name: "ui_inspect",
        schema: SCHEMA_UI_INSPECT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_ui_inspect,
    },
    ToolDef {
        name: "ui_click",
        schema: SCHEMA_UI_CLICK,
        mutating: true,
        claims_patterns: &["已点击", "已按下"],
        max_output_bytes: 8192,
        call: call_ui_click,
    },
    ToolDef {
        name: "ui_type",
        schema: SCHEMA_UI_TYPE,
        mutating: true,
        claims_patterns: &["已输入"],
        max_output_bytes: 8192,
        call: call_ui_type,
    },
    ToolDef {
        name: "wait_for_element",
        schema: SCHEMA_WAIT_FOR_ELEMENT,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_wait_for_element,
    },
    // 受管长驻服务。追加在表尾：同 manage_file 的理由——基线只比前缀。
    ToolDef {
        name: "start_service",
        schema: SCHEMA_START_SERVICE,
        mutating: true,
        claims_patterns: &["已启动", "已启动服务", "服务已启动", "已在后台启动"],
        max_output_bytes: 8192,
        call: call_start_service,
    },
    ToolDef {
        name: "stop_service",
        schema: SCHEMA_STOP_SERVICE,
        mutating: true,
        claims_patterns: &["已停止", "已停止服务", "服务已停止", "已关闭服务"],
        max_output_bytes: 8192,
        call: call_stop_service,
    },
    ToolDef {
        name: "list_services",
        schema: SCHEMA_LIST_SERVICES,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_list_services,
    },
    // 计划工具。追加在表尾：同 manage_file 的理由——基线只比前缀。
    ToolDef {
        name: "update_plan",
        schema: SCHEMA_UPDATE_PLAN,
        mutating: false,
        claims_patterns: &[],
        max_output_bytes: 8192,
        call: call_update_plan,
    },
];
/// TOOLS JSON 由 TOOLS_TABLE 顺序拼装（schema 常量原文直拼，不做 parse + re-serialize）。
/// 少一次运行期解析，也不给「schema 非法 → expect panic 杀聊天」留路径
/// （合法性 + 与 baseline 的一致性由 registry_tests 锁死）。
///
/// 按名字表拼一份工具 JSON：`[` + 每项 `\n  ` + `,\n  ` + `\n]`。
///
/// 三处清单（主 agent 默认 / profile 白名单 / per-onion 交集）共用这一个拼装，
/// 差异只是过滤依据。分开写三份的代价是 `splice_mcp_body` 依赖的「恒以 `\n]`
/// 收尾」成了口头约定——破坏它只能靠限频 eprintln 事后发现（那是运行时降级
/// 告警，本该由抽象收敛消掉）。
///
/// 空集产出 `[\n]`：模型看到零工具时该收到空清单，而不是整份默认清单——
/// 后者是「配置看着生效、实际落空」的相反版本。
fn assemble_tools_json(keep: impl Fn(&ToolDef) -> bool) -> String {
    let mut s = String::with_capacity(16 * 1024);
    s.push('[');
    let mut first = true;
    for tool in TOOLS_TABLE.iter() {
        if !keep(tool) {
            continue;
        }
        s.push_str(if first { "\n  " } else { ",\n  " });
        first = false;
        s.push_str(tool.schema);
    }
    s.push_str("\n]");
    s
}

/// 仅子 agent 可见的工具（write_artifact_file / read_own_card）不出现在
/// 主 agent 的默认清单里——主可见数 = BUILTIN_TOOL_COUNT - SUBAGENT_ONLY_TOOLS.len()。
pub fn tools_json() -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| assemble_tools_json(|t| !SUBAGENT_ONLY_TOOLS.contains(&t.name)))
}

/// 仅子 agent 白名单可见的工具（主 agent 默认 schema 不含）。
pub const SUBAGENT_ONLY_TOOLS: &[&str] = &["write_artifact_file", "read_own_card"];

/// 子 agent 工具白名单（设计 §5.1 → 本仓工具映射）。
/// 禁止（所有 profile）：spawn_subagent / check_subagent / cancel_subagent /
/// 任务卡主状态写（create_task/complete_task/edit_task/toggle_subtask 等全部不在列）/
/// 任务执行 / 权限变更——白名单外一律由 dispatch 闸拒绝。
///
/// `grep_files` + `extract_document` 是只读扩展：Probe profile 的 yaml 要真正
/// 能「搜代码 + 读文档」，否则那份配置看着生效、实际落空（收紧模型里
/// profile 给上限、yaml 再收窄，所以上限不够就永远用不上）。
pub const RESEARCH_TOOLS: &[&str] = &[
    "web_search",
    "fetch_url",
    "list_files",
    "read_text_file",
    "grep_files",
    "extract_document",
    "write_artifact_file",
    "read_own_card",
];
pub const CODER_TOOLS: &[&str] = &[
    "read_text_file",
    "list_files",
    "grep_files",
    "edit_file",
    "write_file",
    "run_python",
    "ast_inspect",
    "unpack_archive",
    "pack_archive",
    "write_artifact_file",
    "read_own_card",
];
pub const GENERAL_TOOLS: &[&str] = &[
    "web_search",
    "fetch_url",
    "read_text_file",
    "list_files",
    "grep_files",
    "extract_document",
    "edit_file",
    "write_file",
    "run_python",
    "ast_inspect",
    "unpack_archive",
    "pack_archive",
    "write_artifact_file",
    "read_own_card",
];

pub fn profile_whitelist(profile: crate::db::SubagentProfile) -> &'static [&'static str] {
    match profile {
        crate::db::SubagentProfile::Research => RESEARCH_TOOLS,
        crate::db::SubagentProfile::Coder => CODER_TOOLS,
        crate::db::SubagentProfile::General => GENERAL_TOOLS,
    }
}

/// 按会话选工具 schema：子 agent 会话 → profile 白名单 JSON；其他 → 默认全量。
/// run_model_loop_core 每轮经此处取 tools（设计 §5.1 白名单 + §10 递归双保险①）。
///
/// 绑了 per-onion 配置的会话走第三态：`运行时 profile ∩ yaml.allowed`。
/// **运行时 profile 优先于 yaml 的 `profile:` 字段**——子 agent 会话的白名单
/// 由派发时的 profile 决定，若让 yaml 的 profile 覆盖就成了「放宽」，与
/// 「只有收紧、没有放宽」直接冲突。yaml 的 `profile` 只在没有子 agent 上下文的
/// 会话上兜底，或作为「这份配置自称哪一档」被校验。
///
/// 返回 `Cow`：无配置时原样借静态串（热路径零分配不变），有配置收窄才现算 owned。
pub fn tools_json_for(session_id: Option<&str>) -> std::borrow::Cow<'static, str> {
    let runtime_profile = session_id
        .and_then(|sid| crate::tool_guard::subagent_ctx(Some(sid)))
        .map(|c| c.profile);
    let cfg = crate::bot_onions::cached_for_session(session_id);
    // 上限：运行时 profile 优先；都没有时用 yaml 自称的 profile 兜底
    let profile = match (runtime_profile, cfg.as_ref().and_then(|c| c.profile_enum())) {
        (Some(p), _) => Some(p),
        (None, Some(p)) => Some(p),
        (None, None) => None,
    };
    let Some(profile) = profile else {
        return std::borrow::Cow::Borrowed(tools_json());
    };
    let allowed = cfg.as_ref().map(|c| c.allowed.as_slice()).unwrap_or(&[]);
    let effective = crate::bot_onions::effective_tools(profile, allowed);
    // 没有额外收窄 → 沿用按 profile 缓存的静态串，不重算
    if effective.len() == profile_whitelist(profile).len() {
        return std::borrow::Cow::Borrowed(profile_tools_json(profile));
    }
    std::borrow::Cow::Owned(narrowed_tools_json(&effective))
}

/// 主 agent 的完整工具清单（阶段 3 MCP 挂载点，机制 A）：
/// 内置静态 JSON 尾部追加外部 MCP 工具（增量挂载，内置 BUILTIN_TOOL_COUNT 个工具的 schema 字节不动）。
/// - 子 agent 会话：短路返回白名单（外部 MCP 工具不进子 agent，§5.1 边界不破）；
/// - 无 MCP 连接：原样返回静态 &'static str（零分配，热路径不变）；
/// - 有连接：Owned String = 静态 JSON 摘尾 + `mount::mcp_tools_json_body()` + 收尾。
/// 消费点 bot_model_loop 对拼装结果 fail-soft：schema 构建层保证 JSON
/// 合法（mount/registry 单测锁），解析失败记审计走空工具表，不再 panic。
pub fn tools_json_with_mcp(session_id: Option<&str>) -> std::borrow::Cow<'static, str> {
    // 子 agent 判定显式化（评审：原 ptr::eq 指针比较对 tools_json_for 的
    // 返回形态有隐式契约，重构易碎）——subagent_ctx 是内存查表，重复一次开销可忽略
    let is_subagent = session_id
        .and_then(|sid| crate::tool_guard::subagent_ctx(Some(sid)))
        .is_some();
    let base = tools_json_for(session_id);
    if is_subagent {
        // 子 agent 白名单：静态借用返回
        return base;
    }
    let body = crate::bot::mcp::mount::mcp_tools_json_body();
    if body.is_empty() {
        return base;
    }
    // base 恒以 "\n]" 收尾（tools_json 拼装格式）。契约破坏属"永不发生"态：
    // 每轮模型循环都走这里，告警限频防刷屏——第 1 次 + 之后每 100 次
    //（评审：OnceLock 只告一次，持续破坏会彻底静默）；回退 base 后
    // 消费点的 fail-soft 兜底接住（release 无 debug_assert，不产出非法 JSON）。
    static CONTRACT_BREAKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if !base.ends_with("\n]") {
        let n = CONTRACT_BREAKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n == 1 || n.is_multiple_of(100) {
            eprintln!(
                "[registry] tools_json 格式契约破坏（未以 \\n] 收尾，第 {n} 次），MCP 工具本轮不挂载"
            );
        }
    }
    // base 可能是 owned（本会话绑了 onion 配置、清单是现算的交集），
    // 那种情况下没法借出去，只能再拷一份收尾。两版共用同一个拼接内核
    match base {
        std::borrow::Cow::Borrowed(b) => splice_mcp_body(b, &body),
        std::borrow::Cow::Owned(o) => std::borrow::Cow::Owned(splice_mcp_body_owned(o, &body)),
    }
}

/// 拼接内核（纯函数，单测直打）：base 恒以 `"\n]"` 收尾、动态段恒以
/// `",\n"` 起头（build_tools_json_body 构造保证），摘尾接段再补收尾。
///
/// 契约破坏返回 None，调用方各自回退原样 base（借用版回退 Borrowed、
/// owned 版回退原串）——宁可本轮少挂 MCP 工具，不产出非法 JSON。
/// 两版拼接共用这一个内核，判定逻辑只此一份。
fn join_mcp_body(base: &str, body: &str) -> Option<String> {
    let head = base.strip_suffix("\n]")?;
    if !body.starts_with(",\n") {
        return None;
    }
    let mut s = String::with_capacity(head.len() + body.len() + 2);
    s.push_str(head);
    s.push_str(body);
    s.push_str("\n]");
    Some(s)
}

pub(super) fn splice_mcp_body<'a>(base: &'a str, body: &str) -> std::borrow::Cow<'a, str> {
    match join_mcp_body(base, body) {
        Some(s) => std::borrow::Cow::Owned(s),
        None => std::borrow::Cow::Borrowed(base),
    }
}

/// `splice_mcp_body` 的 owned 版：base 已持有所有权时借不出去，只能再拷一份。
/// 契约判断复用同一个 `join_mcp_body` 内核，破坏时回退原串。
pub(super) fn splice_mcp_body_owned(base: String, body: &str) -> String {
    join_mcp_body(&base, body).unwrap_or(base)
}

pub fn profile_tools_json(profile: crate::db::SubagentProfile) -> &'static str {
    static CACHE: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let mut map = HashMap::new();
            for p in [
                crate::db::SubagentProfile::Research,
                crate::db::SubagentProfile::Coder,
                crate::db::SubagentProfile::General,
            ] {
                let whitelist = profile_whitelist(p);
                map.insert(
                    p.as_str(),
                    assemble_tools_json(|t| whitelist.contains(&t.name)),
                );
            }
            map
        })
        .get(profile.as_str())
        .map(|s| s.as_str())
        .unwrap_or_else(|| tools_json())
}

/// 按 `effective` 名字表拼一份收窄后的工具 JSON（owned）。
/// 过滤依据是「profile 白名单 ∩ per-onion allowed」的交集结果，
/// 拼装形状与 `tools_json` 同源（`assemble_tools_json`）。
pub(super) fn narrowed_tools_json(effective: &[&'static str]) -> String {
    assemble_tools_json(|t| effective.contains(&t.name))
}

/// MUTATING_TOOLS 由 TOOLS_TABLE 过滤（mutating == true）。
pub fn mutating_tools() -> Vec<&'static str> {
    TOOLS_TABLE
        .iter()
        .filter(|t| t.mutating)
        .map(|t| t.name)
        .collect()
}
