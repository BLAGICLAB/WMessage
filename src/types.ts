// 单一来源契约：与后端 `crate::db::TaskStatus` (src-tauri/src/db/tasks.rs) 一一对应。
// 后端 wire format 是 lowercase 字符串 (serde 自定义实现),前端 union 必须同步;
// 任何新增/删减 status 都需要同步改 Rust enum + 前端 union + DB 列约束 + 注册表 JSON schema。
// 漂移会在 serde 边界静默失败(以 400 status 错或 column 校验失败形式出现)。
export type ColumnId = "todo" | "doing" | "done";

/** 工作区静态链接（与 Rust db::WorkspaceLink 对应） */
export interface WorkspaceLink {
  id: string;
  /** 展示别名，用户可自由自定义 */
  displayName: string;
  /** 底层真实路径 / 网页链接 */
  targetUri: string;
  /** url | file | folder */
  kind: "url" | "file" | "folder";
}

/** 工作区条目：类似任务卡，标题 + 折叠 + 链接列表（与 Rust db::WorkspaceItem 对应） */
export interface WorkspaceItem {
  id: string;
  title: string;
  collapsed?: boolean;
  links: WorkspaceLink[];
  order?: number;
  updatedAt?: number;
}

/** 桌面清理规则（与 Rust migration::MigrationRule 对应） */
export interface MigrationRule {
  id: string;
  enabled: boolean;
  /** 文件名关键字（命中任一即匹配，大小写不敏感） */
  keywords: string[];
  /** move 移动归档 | delete 删除文件（默认关闭，需手动启用） */
  action: "move" | "delete";
  /** 归档目录：相对桌面；{year} 展开为年份；绝对路径原样 */
  archiveDir: string;
}

/** 迁移执行报告（与 Rust migration::MigrationReport 对应） */
export interface MigrationReport {
  ts: number;
  archived: number;
  moved: number;
  deleted: number;
  skipped: number;
  log: string[];
  /** 是否被用户取消而提前停止（可选：后端恒发，前端向后兼容标记） */
  cancelled?: boolean;
}

interface Subtask {
  id: string;
  text: string;
  done: boolean;
}

/** 画布坐标（工作流卡专用，与 Rust db::CanvasPos 对应） */
interface CanvasPos {
  x: number;
  y: number;
}

export interface Task {
  id: string;
  title: string;
  /** "YYYY-MM-DD"（旧）或 "YYYY-MM-DDTHH:mm" */
  due?: string;
  /** 标题下的小字备注 */
  note?: string;
  /** 标签列表 */
  tags?: string[];
  /** 绑定文件列表（上限 10；isDir=true 为文件夹，文件夹仍单选独占） */
  files?: Array<{ path: string; isDir: boolean }>;
  /** 旧单绑定字段：迁移过渡保留（启动时若 files 为空自动迁入 files）。   * null = 显式清空（task_patch 语义：null 落库清值，缺键 = 未提供保留旧值）；
   * 直连 invoke 构造清空 patch 时必须传 null，不要传 undefined（会被 JSON 丢弃） */
  filePath?: string | null;
  /** 绑定的是否为文件夹（旧字段，见 filePath；null 语义同上） */
  fileIsDir?: boolean | null;
  /** 完成时间（epoch ms） */
  completedAt?: number;
  /** 创建时间（epoch ms）；老数据缺省 = 未知（图谱「耗时」口径的依赖字段） */
  createdAt?: number;
  /** 已归档：从「完成」列隐藏，可在归档视图查看/恢复 */
  archived?: boolean;
  /** 已删除（软删除进回收站） */
  deletedAt?: number;
  column: ColumnId;
  subtasks?: Subtask[];
  /** 标题以下内容是否折叠（主窗口/挂件共享，默认展开） */
  collapsed?: boolean;
  /** 拖拽排序用全局序号（越小越靠前） */
  order?: number;
  /** 最后修改时间（epoch ms），合并导入时同 id 取更新者 */
  updatedAt?: number;
  /**
   * RMW 写回基线 = 读快照时该行的 updatedAt。
   * 仅随 db_upsert 上行（后端不落库、不在事件/导出中下发）；后端写前比对现行行，
   * 不一致 → 冲突拒写（防整行覆盖 lost-update）。新建/未读快照的写不带此字段。
   */
  expectedUpdatedAt?: number;
  /** 已交给机器人执行（🤖 点击置真，执行结束无论成败清除）。
   *  头像规则：botAssigned 或 schedule 任一存在 → 机器人头像；否则用户头像 */
  botAssigned?: boolean;
  /** 定时执行规则：daily:HH:MM / weekly:D:HH:MM / at:YYYY-MM-DDTHH:MM（设置期间一直显示机器人头像） */
  schedule?: string | null;
  /** 上次定时执行时间（epoch ms） */
  schedLast?: number | null;
  /** 定时启用开关：undefined = 启用（暂停保留配置不丢节奏） */
  enabled?: boolean;
  /** 计划开始（周时间网格排期）："YYYY-MM-DDTHH:mm"（与 due 同格式）；缺省 = 未排期。
   *  null = 显式清除（task_patch 语义：null 落库清值，缺键 = 未提供保留旧值） */
  planStart?: string | null;
  /** 计划结束，恒 > planStart；null 语义同上 */
  planEnd?: string | null;
  /** 子 agent 编排（ 投影）：串链键 = 子 agent 执行会话 id（不直接展示） */
  assignee?: string | null;
  /** 预算三硬顶（上卡可见）：轮数 / 工具调用 / 墙钟秒 */
  budget?: {
    maxTurns: number;
    maxToolCalls: number;
    maxWallSeconds: number;
  } | null;
  /** 收尾结构化结果（设计 §7）：卡片折叠展示 */
  result?: {
    status?: string;
    summary?: string;
    artifacts?: Array<{ path?: string; description?: string }>;
    blockers?: unknown[];
    confidence?: number;
    /** 节点级验收裁决（pass/partial/fail/unknown；fail 已终态 failed，不会以 success 出现） */
    acceptanceVerdict?: string;
    /** 验收依据（≤100 字，TracePanel 展示） */
    acceptanceEvidence?: string;
    /** 含返工的累计执行次数（引擎写） */
    attempt?: number;
    /** 本卡耗时（墙钟 ms，引擎写） */
    ms?: number;
  } | null;
  /** 工作流画布归属（设计 §3.1）：缺省 "user" = 看板任务；
   *  "workflow" = 工作流节点卡（看板/挂件默认过滤，bot 工具不过滤） */
  origin?: "user" | "workflow";
  /** 所属工作流 id（origin="workflow" 时有值） */
  workflowId?: string;
  /** 上游任务 id 列表（DAG 依赖 = 画布连线；工作流卡专用） */
  dependsOn?: string[];
  /** 画布坐标（仅工作流卡使用） */
  canvasPos?: CanvasPos;
  /** 执行用大模型：模型库条目 id；缺省 = 跟随全局 active 模型 */
  model?: string;
  /** 每卡验收标准（W-QA 卡即契约）：拆解生成的一行可验证完成标准；执行时注入并自检 */
  acceptance?: string | null;
  /**
   * 归属人 personId（任务图谱设计 §1.1）：undefined = 本人。
   * 导入多人数据后，外来任务带其原主人的 pid；看板/归档/回收站/⌘K 默认只显示
   * undefined（本人）的卡，图谱/统计看全部。
   */
  ownerId?: string;
}

/** 成员条目（people 表视图，任务图谱设计 §1.2）：isSelf 行 name 由 profile 现值合并 */
export interface PeopleEntry {
  id: string;
  name: string;
  isSelf: boolean;
}

/** 工作流元数据（与 Rust db::Workflow 对应，存 workflows 表；节点 = origin="workflow" 的任务卡） */
export interface Workflow {
  id: string;
  name: string;
  /** 用户原始自然语言总目标（总目标卡展示文本） */
  goal: string;
  createdAt?: number;
  updatedAt?: number;
  /** 定时执行规则（同 Task.schedule 格式）：null/undefined = 未设 */
  schedule?: string | null;
  /** 上次定时执行时间（epoch ms） */
  schedLast?: number | null;
  /** 定时启用开关：undefined = 启用 */
  enabled?: boolean;
  /** 上轮执行的收尾审校报告（W-QA，JSON 字符串：verdict/overall/issues）；null = 从未评审 */
  lastReport?: string | null;
  /** 报告写入时间（epoch ms） */
  lastReportAt?: number | null;
  /** 澄清元数据JSON 串：{answers:[{question,answer}], askMode}）；null = 从未澄清 */
  clarifyMeta?: string | null;
}

/** 评审裁决四值（Rust 契约）：pass/partial/fail = rubric 裁决；unknown = 评审 * 调用/解析失败降级。注意后端 ReviewReport.verdict 是 String 直传（模型输出
 * 契约外字符串现实可达），前端在解析边界归一成这四值后才落入本类型 */
export type ReviewVerdict = "pass" | "partial" | "fail" | "unknown";

/** 工作流收尾审校报告（W-QA，runner rubric 评审产出；workflow-report 事件与 lastReport 同构） */
export interface WorkflowReport {
  verdict: ReviewVerdict;
  /** 两三句整体结论（降级时为评审原文） */
  overall: string;
  issues: Array<{
    nodeTitle: string;
    problem: string;
    /** true = 评审判定需重做该节点（runner 已据此触发一轮有界返工） */
    needsRework?: boolean;
  }>;
  /** true = 返工后的轻量终审（只覆盖返工节点） */
  reworkRound?: boolean;
  reworkedNodes?: string[];
}

/** schedule_overview 返回的条目（定时作业/工作流两源合并，字段与 Rust 侧 camelCase 对齐） */
export interface ScheduleEntry {
  /** 目标类型：job = 定时作业（到点按内容新建任务卡交机器人）；workflow = 已有工作流模板 */
  kind: "job" | "workflow";
  /** 目标 id（作业 id / 工作流 id） */
  targetId: string;
  /** 作业内容（即「要做什么」）/ 工作流名 */
  title: string;
  /** 摘要（工作流 goal；作业恒为 null） */
  detail: string | null;
  /** 定时规则串（daily:/weekly:/monthly:/at:） */
  schedule: string;
  /** 上次定时执行时间（epoch ms），未跑过为 null */
  schedLast: number | null;
  /** 下次执行时间（epoch ms，Rust 端单源计算，前端不解析 schedule） */
  nextRunAt: number | null;
  /** 已错过（超 2h 补跑窗口被跳过，Rust 端判定） */
  missed: boolean;
  /** 启用开关：false = 已暂停（保留配置） */
  enabled: boolean;
  /** 最近一次执行结果（"ok"|"fail"，null=从未跑）；仅 job 条目有意义，workflow 恒 null */
  lastStatus?: string | null;
  /** 最近失败原因摘要（≤300 字）；仅 job 条目有意义 */
  lastError?: string | null;
  /** 失败重试次数上限（0=不重试；失败 5 分钟后自动重试，用尽才算最终失败） */
  retryMax: number;
  /** 重试用尽仍失败时自动暂停此任务并通知 */
  pauseOnFailure: boolean;
}

/** scheduled_job_history 返回的单次执行记录（倒序，最新在前；与 Rust 侧 camelCase 对齐） */
export interface ScheduledJobRun {
  id: string;
  /** 所属作业 id */
  jobId: string;
  /** 触发时间（epoch ms） */
  firedAt: number;
  /** 执行结果 */
  status: "ok" | "fail";
  /** 耗时（ms） */
  durationMs: number;
  /** 结果摘要（失败时为原因摘要） */
  summary: string | null;
  /** 本次执行新建的任务卡 id（前端可跳卡片，v1 先不接线） */
  cardId: string | null;
}

/** workflow_save 结果：本地节点 id → 真实任务 id 的绑定（画布据此挂接任务卡） */
interface WorkflowSaveBinding {
  /** 画布草稿节点本地 id */
  localId: string;
  /** 落库后的真实任务 id（保留的旧卡 = 原 id；新卡 = 新 uuid） */
  taskId: string;
  /** 是否新建（false = 指纹命中保留原卡） */
  created: boolean;
}

export interface WorkflowSaveResult {
  workflowId: string;
  bindings: WorkflowSaveBinding[];
  kept: number;
  created: number;
  deleted: number;
}
