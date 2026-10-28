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
}

/** 扫描整理来源（与 Rust migration::ScanSource 对应） */
export interface ScanSource {
  /** "desktop"（预设，path 忽略，运行时解析真实桌面）| "custom"（用户所选绝对路径） */
  kind: "desktop" | "custom";
  /** custom 的文件夹绝对路径；desktop 恒为空 */
  path: string;
  /** 归档根：相对 path；空 = 文件夹本身 */
  archiveRoot: string;
  enabled: boolean;
}

/** 来源表（与 Rust migration::SourcesFile 对应） */
export interface SourcesFile {
  sources: ScanSource[];
}

export interface Subtask {
  id: string;
  text: string;
  done: boolean;
  /** 该子任务行一键生成的任务卡 id（行上 ↗ 跳转标记的指向）；缺省 = 未生成 */
  taskId?: string;
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
  /** 标题以下内容是否折叠（默认展开） */
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
   *  头像规则：botAssigned 存在 → 机器人头像；否则用户头像 */
  botAssigned?: boolean;
  /** 计划开始（周时间网格排期）："YYYY-MM-DDTHH:mm"（与 due 同格式）；缺省 = 未排期。
   *  null = 显式清除（task_patch 语义：null 落库清值，缺键 = 未提供保留旧值） */
  planStart?: string | null;
  /** 计划结束，恒 > planStart；null 语义同上 */
  planEnd?: string | null;
  /** 子任务卡出身：子任务行一键生成的卡记母卡 id；普通卡缺省。
   *  仅作关联键（详情面板 ↗ 跳转、母卡删除连带询问），无级联语义；
   *  经 db_upsert 整行写入，不走 task_patch（白名单不含此字段） */
  parentId?: string | null;
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
  } | null;
  /** 上游任务 id 列表（依赖边；图谱/KPI 关联簇数据源） */
  dependsOn?: string[];
  /** 执行用大模型：模型库条目 id；缺省 = 跟随全局 active 模型 */
  model?: string;
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

/** schedule_overview 返回的条目（定时作业，字段与 Rust 侧 camelCase 对齐） */
export interface ScheduleEntry {
  /** 目标类型：job = 定时作业（到点按内容新建任务卡交机器人） */
  kind: "job";
  /** 作业 id */
  targetId: string;
  /** 作业内容（即「要做什么」） */
  title: string;
  /** 摘要（作业恒为 null） */
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
  /** 最近一次执行结果（"ok"|"fail"，null=从未跑） */
  lastStatus?: string | null;
  /** 最近失败原因摘要（≤300 字） */
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
