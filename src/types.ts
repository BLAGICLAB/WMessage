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

interface Subtask {
  id: string;
  text: string;
  done: boolean;
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
  /** 旧单绑定字段：迁移过渡保留（启动时若 files 为空自动迁入 files） */
  filePath?: string;
  /** 绑定的是否为文件夹（旧字段，见 filePath） */
  fileIsDir?: boolean;
  /** 完成时间（epoch ms） */
  completedAt?: number;
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
}
