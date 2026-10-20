// 程序模块 Tauri 桥（后端 `crate::apps::commands`）
//
// 入参/出参字段名（camelCase）= 后端 serde camelCase 直出。AI 上架时的
// 入口契约见 `docs/APP-DEV-CONVENTIONS.md`。

import { invoke } from "@tauri-apps/api/core";

/** 程序元数据（与 Rust `db::apps::App` 对应，serde camelCase 直出） */
export interface App {
  id: string;
  name: string;
  description: string;
  icon: string;
  /**
   * 输入声明 JSON 字符串（保持原始 JSON 文本，前端按需 JSON.parse）：
   * - 文件夹模式 `{"kind":"folder"}`（约定 `data/ input/ output/`，无字段）
   * - 表单模式 `{"kind":"form","fields":[{"key","label","type","required","placeholder"}]}`
   */
  inputSchema: string;
  /** `{data_dir}/apps/<id>/` 之下的相对路径（M1 恒为 `app.py`） */
  scriptPath: string;
  manual: string;
  status: "draft" | "enabled" | "archived";
  enabledAt: number | null;
  createdAt: number;
  updatedAt: number;
  /**
   * 声明的单次运行时长上限（秒）；null = 未声明（运行时用内置默认 60s）。
   * 耗时随输入规模增长的程序（转写长录音、装依赖、批处理）必须声明，
   * 否则会在默认时长被中途掐掉。写侧已钳到后端天花板（落盘值 = 生效值）。
   */
  timeoutSecs: number | null;
}

/** 单次运行元数据（与 Rust `db::apps::AppRun` 对应） */
export interface AppRun {
  id: number;
  appId: string;
  startedAt: number;
  finishedAt: number | null;
  durationMs: number | null;
  status: string;
  summary: string | null;
}

/** 一次运行的结果（runner 直出） */
export interface AppRunResult {
  runId: number;
  appId: string;
  /** 产物目录：`{data_dir}/apps/<id>/output/<run_id>/` */
  outputDir: string;
  /** 哨兵 `##RESULT## {...}` 解析出的结构化对象；无哨兵 = None */
  resultJson: Record<string, unknown> | null;
  /** 哨兵里的 `summary` 字段（无哨兵时 = `resultJson` 序列化）；通知用 */
  summary: string | null;
  stdout: string;
  stderr: string;
  exitCode: number | null;
  truncated: boolean;
  durationMs: number;
}

/** 秒 → 人读时长（整分钟走分钟，否则走秒）。展示用；秒数本身来自后端。 */
export function fmtTimeoutSecs(secs: number): string {
  return secs % 60 === 0 ? `${secs / 60} 分钟` : `${secs} 秒`;
}

/** 程序页 M2 全量视图（草稿 + 已启用，归档不返回），按 updated_at 倒序 */
export function listActiveApps(): Promise<App[]> {
  return invoke<App[]>("apps_list_active");
}

/** 已归档程序（程序页「已归档」视图的数据源） */
export function listArchivedApps(): Promise<App[]> {
  return invoke<App[]>("apps_list_archived");
}

/** 归档：程序页用户动作，bot 工具不受理。脚本、数据、版本快照、运行历史
 *  原地保留，只是不再出现在程序页——所以它是可逆的，不是删除。
 *  重复点幂等。 */
export function archiveApp(id: string): Promise<App> {
  return invoke<App>("apps_archive", { id });
}

/** 恢复归档程序 → 草稿态：回到程序页可试跑，重新上架由用户满意后再定 */
export function unarchiveApp(id: string): Promise<App> {
  return invoke<App>("apps_unarchive", { id });
}

/** 彻底删除归档程序：库内三张表的行 + 程序目录全部抹掉，**不可逆**。
 *  后端只受理已归档程序——归档视图里那个删除键是唯一入口。
 *  `data/`（AI 预置）、`input/`（你自己放的数据）、`output/`（产物）都在目录内，
 *  一并删除，不留任何东西。 */
export function deleteApp(id: string): Promise<void> {
  return invoke<void>("apps_delete", { id });
}

/** 跑一个 app。
 * - 文件夹模式：`params` = null
 * - 表单模式：`params` = 字段 JSON 串（与 `inputSchema` 的 `fields` 顺序一致）
 * - 超时：不传 = 用程序声明的时长（未声明则 60s）；传值 = 本次运行覆盖声明。
 *   最终值由后端钳到全局天花板 */
export function runApp(
  id: string,
  params: Record<string, unknown> | null,
  timeoutSecs?: number,
): Promise<AppRunResult> {
  return invoke<AppRunResult>("apps_run", {
    args: {
      id,
      params: params ? JSON.stringify(params) : null,
      timeoutSecs: timeoutSecs ?? null,
    },
  });
}

/** 上架。`app.py` 等脚本必须已先落在 `{data_dir}/apps/<id>/` 下；
 *  命令只登记元数据 + 检查脚本可读。
 *  status 推荐传 'enabled'（M1 阶段用，不存在 draft 流程） */
export interface CreateAppArgs {
  id: string;
  name: string;
  description: string;
  icon: string;
  inputSchema: string;
  scriptPath: string;
  manual: string;
  status: "draft" | "enabled" | "archived";
  /** 声明的单次运行时长（秒）；不传/0 = 未声明（运行时用内置默认 60s） */
  timeoutSecs?: number | null;
}
export function createApp(args: CreateAppArgs): Promise<App> {
  return invoke<App>("apps_create", { args });
}

/** 最近运行历史（程序页 + 管理面板） */
export function listAppRuns(appId: string, limit = 20): Promise<AppRun[]> {
  return invoke<AppRun[]>("apps_runs_list", { appId, limit });
}

/** 版本快照元数据（列表行，不含脚本全文） */
export interface AppVersionMeta {
  id: number;
  appId: string;
  version: number;
  name: string;
  icon: string;
  /** create / update / restore */
  source: string;
  createdAt: number;
}

/** 版本快照列表（新→旧） */
export function listAppVersions(appId: string, limit = 100): Promise<AppVersionMeta[]> {
  return invoke<AppVersionMeta[]>("apps_versions_list", { appId, limit });
}

/** 回滚到历史版本：内容整体回旧版，回滚本身记为新版本（可再回滚撤销） */
export function restoreAppVersion(appId: string, version: number): Promise<App> {
  return invoke<App>("apps_version_restore", { appId, version });
}

/** 在系统文件管理器中打开程序的 output/ 目录（不存在则先建）。
 *  返回打开的目录绝对路径 */
export function openAppOutput(appId: string): Promise<string> {
  return invoke<string>("apps_open_output", { id: appId });
}

/** 解析 inputSchema JSON 串为对象（解析失败 = null，让 UI 走文件夹模式兜底） */
export function parseInputSchema(
  raw: string,
):
  | { kind: "folder" }
  | { kind: "form"; fields: AppInputField[] }
  | null {
  try {
    const v = JSON.parse(raw) as { kind?: string; fields?: AppInputField[] };
    if (v?.kind === "folder") return { kind: "folder" };
    if (v?.kind === "form" && Array.isArray(v.fields)) {
      return { kind: "form", fields: v.fields };
    }
    return null;
  } catch {
    return null;
  }
}

export interface AppInputField {
  key: string;
  label: string;
  type: "text" | "number" | "date";
  required?: boolean;
  placeholder?: string;
}
