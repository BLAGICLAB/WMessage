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

/** 程序页 M2 全量视图（草稿 + 已启用，归档不返回），按 updated_at 倒序 */
export function listActiveApps(): Promise<App[]> {
  return invoke<App[]>("apps_list_active");
}

/** 跑一个 app。
 * - 文件夹模式：`params` = null
 * - 表单模式：`params` = 字段 JSON 串（与 `inputSchema` 的 `fields` 顺序一致）
 * - 超时：默认 60s（与 py::run_python 同款）；可调但实际 M1 不用 */
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
