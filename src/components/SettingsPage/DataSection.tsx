/** 设置页「data」分区的内容块（从 SettingsPage 主组件搬出）。
 *
 *  挂载与可见性由父组件的 <section hidden={...}> 负责——各分区面板是
 *  同时挂载的（hidden 切可见性），这里只管内容。
 */
import { DEFAULT_ARCHIVE_DAYS, MAX_ARCHIVE_DAYS } from "../../lib/archiveRule";
import { HomeKpiPanel } from "./HomeKpiPanel";
import { SettingRow } from "./SettingRow";
import { Download, Upload } from "lucide-react";
import type { Dispatch, JSX, SetStateAction } from "react";
import type { SettingsConfig } from "./types";

export type DataSectionProps = {
  exporting: boolean;
  runExport: () => Promise<void>;
  importing: boolean;
  runImport: () => Promise<void>;
  deleteNonSelfBusy: boolean;
  deleteNonSelfDone: string | null;
  runDeleteNonSelf: () => Promise<void>;
  config: SettingsConfig;
  setConfig: Dispatch<SetStateAction<SettingsConfig>>;
  renderSaveButton: (cls: string) => JSX.Element;
  traceRetentionDays: string | undefined;
  traceRetainDays: string;
  setTraceRetainDays: Dispatch<SetStateAction<string>>;
  traceRetentionTitle: string;
  traceClearBusy: boolean;
  clearTraces: () => Promise<void>;
  traceClearMsg: string;
  exportingWs: boolean;
  runExportWs: () => Promise<void>;
  importingWs: boolean;
  runImportWs: () => Promise<void>;
};

export function DataSection({
  exporting,
  runExport,
  importing,
  runImport,
  deleteNonSelfBusy,
  deleteNonSelfDone,
  runDeleteNonSelf,
  config,
  setConfig,
  renderSaveButton,
  traceRetentionDays,
  traceRetainDays,
  setTraceRetainDays,
  traceRetentionTitle,
  traceClearBusy,
  clearTraces,
  traceClearMsg,
  exportingWs,
  runExportWs,
  importingWs,
  runImportWs,
}: DataSectionProps) {
  return (
    <>
      {/* 任务数据管理 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">
          任务数据管理
        </h2>
        <div className="mt-4 space-y-3">
          <SettingRow
            label="导出任务数据"
            description="全部任务卡（含归档、回收站）导出为 JSON 文件"
          >
            <button
              className={`btn-sm-action ${
                exporting ? "nm-inset" : "nm-outset"
              }`}
              onClick={runExport}
              disabled={exporting}
            >
              {exporting ? (
                "导出中…"
              ) : (
                <>
                  <Download size={12} aria-hidden /> 导出
                </>
              )}
            </button>
          </SettingRow>
          <SettingRow
            label="导入任务数据"
            description="从 JSON 文件导入任务卡；按 id 合并，同 id 保留最后修改的记录"
          >
            <button
              className={`btn-sm-action ${
                importing ? "nm-inset" : "nm-outset"
              }`}
              onClick={runImport}
              disabled={importing}
            >
              {importing ? (
                "导入中…"
              ) : (
                <>
                  <Upload size={12} aria-hidden /> 导入
                </>
              )}
            </button>
          </SettingRow>
          {/* 删除非本人的任务卡：导入带来的归属人为他人的统计用卡，
      一键直接硬删（含回收站中的；本人 = 个人资料 personId，owner_id 为空的卡视为本人不动）。
      后端广播 tasks-updated 三端同步；结果在按钮文字上短暂反馈（不新增行防抖动） */}
          <div className="pt-3 border-t border-[var(--edge)] flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">
                删除非本人的任务卡
              </p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                立即删除归属人为他人的全部任务卡（含回收站，不可恢复）；本人卡与未设归属人的卡不受影响
              </p>
            </div>
            <button
              className={`shrink-0 min-w-[110px] px-4 py-1.5 text-sm inline-flex items-center justify-center gap-1.5 whitespace-nowrap ${
                deleteNonSelfBusy ? "nm-inset" : "nm-outset"
              } ${deleteNonSelfDone ? "text-[var(--t4)]" : "text-[var(--danger)]"}`}
              onClick={() => void runDeleteNonSelf()}
              disabled={deleteNonSelfBusy}
            >
              {deleteNonSelfBusy
                ? "删除中…"
                : deleteNonSelfDone
                  ? deleteNonSelfDone
                  : "直接删除"}
            </button>
          </div>
          {/* 任务卡归档时间：完成满 N 天自动归档。前端看板规则与后端 migration
      兜底归档（主窗口关闭时照常到期）同读 bot-config.json，改完阈值两侧一致 */}
          <div className="pt-3 border-t border-[var(--edge)] flex items-center justify-between gap-4">
            <div className="min-w-0">
              <p className="text-sm font-medium text-[var(--t2)]">
                任务卡归档时间
              </p>
              <p className="mt-1 text-xs text-[var(--t5)]">
                任务完成超过该天数后自动归档，从「完成」列隐藏；默认{" "}
                {DEFAULT_ARCHIVE_DAYS} 天，上限 {MAX_ARCHIVE_DAYS} 天
              </p>
            </div>
            <div className="shrink-0 flex items-center gap-2">
              <input
                value={config.archiveAfterDays}
                onChange={(e) =>
                  setConfig((c) => ({
                    ...c,
                    archiveAfterDays: e.target.value,
                  }))
                }
                placeholder={String(DEFAULT_ARCHIVE_DAYS)}
                inputMode="numeric"
                aria-label="任务卡归档天数"
                className="nm-inset w-20 rounded-xl px-3 py-1.5 text-xs text-[var(--t3)] outline-none text-right"
              />
              <span className="text-xs text-[var(--t5)]">天</span>
            </div>
          </div>
        </div>
        {/* 归档天数等 setConfig 字段靠本卡保存钮落盘（同机器人卡模式） */}
        {renderSaveButton("mt-3 flex justify-end")}
      </div>

      {/* 首页 KPI 指标：8 选 4 + 调序，localStorage 即点即生效（首页事件同步） */}
      <HomeKpiPanel />

      {/* 执行痕迹清理（trace 三表 + 回滚快照保留期；trace_clear_before 命令） */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">
          执行痕迹
        </h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          {traceRetentionDays
            ? `任务卡/定时/工作流执行的痕迹（工具调用时间线、文件
            diff、回滚快照）落本地库，默认保留 ${traceRetentionDays} 天`
            : `任务卡/定时/工作流执行的痕迹（工具调用时间线、文件
            diff、回滚快照）落本地库，按保留期自动清理`}
        </p>
        <div className="mt-4 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-sm font-medium text-[var(--t2)]">
              清理执行痕迹
            </p>
            <p className="mt-1 text-xs text-[var(--t5)]">
              删除「已收尾超过指定天数」与「挂起超过指定天数」的痕迹（含子表与过期回滚快照引用）；不影响任务数据
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            <input
              aria-label="保留天数"
              value={traceRetainDays}
              onChange={(e) =>
                setTraceRetainDays(e.target.value.replace(/\D/g, ""))
              }
              inputMode="numeric"
              placeholder={traceRetentionDays ?? ""}
              className="w-16 rounded-[var(--r-sm)] nm-inset px-2 py-1 text-right font-mono text-xs text-[var(--t2)] outline-none"
              title={traceRetentionTitle}
            />
            <span className="text-xs text-[var(--t5)]">天前</span>
            <button
              className={`btn-sm-action ${
                traceClearBusy ? "nm-inset" : "nm-outset"
              }`}
              onClick={clearTraces}
              disabled={traceClearBusy}
            >
              {traceClearBusy ? "清理中…" : "清理"}
            </button>
          </div>
        </div>
        {traceClearMsg && (
          <p className="mt-2 text-xs text-[var(--t4)]">
            {traceClearMsg}
          </p>
        )}
      </div>

      {/* 工作区管理：与任务数据管理风格一致；workspace_items 数据独立于任务数据 */}
      <div className="nm-card p-5">
        <h2 className="text-lg font-semibold text-[var(--t1)]">
          工作区管理
        </h2>
        <p className="mt-1 text-xs text-[var(--t5)]">
          工作区是按用途分组的链接集合（文件路径 / 文件夹 /
          网页），与任务数据分开独立存储
        </p>
        <div className="mt-4 space-y-3">
          <SettingRow
            label="导出工作区链接"
            description="全部工作区条目（含分组、折叠状态、链接列表）导出为 JSON 文件"
          >
            <button
              className={`btn-sm-action ${
                exportingWs ? "nm-inset" : "nm-outset"
              }`}
              onClick={runExportWs}
              disabled={exportingWs}
            >
              {exportingWs ? (
                "导出中…"
              ) : (
                <>
                  <Download size={12} aria-hidden /> 导出
                </>
              )}
            </button>
          </SettingRow>
          <SettingRow
            label="导入工作区链接"
            description="从 JSON 文件导入工作区条目；按 id 合并，同 id 保留最后修改的记录"
          >
            <button
              className={`btn-sm-action ${
                importingWs ? "nm-inset" : "nm-outset"
              }`}
              onClick={runImportWs}
              disabled={importingWs}
            >
              {importingWs ? (
                "导入中…"
              ) : (
                <>
                  <Upload size={12} aria-hidden /> 导入
                </>
              )}
            </button>
          </SettingRow>
        </div>
      </div>
    </>
  );
}
