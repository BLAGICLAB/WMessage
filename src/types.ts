export type ColumnId = "todo" | "doing" | "done";

export interface Subtask {
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
  filePath?: string;
  /** 绑定的是否为文件夹 */
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
}
