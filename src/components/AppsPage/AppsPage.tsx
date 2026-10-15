import { useCallback, useEffect, useState } from "react";
import {
  Archive,
  ArchiveRestore,
  Boxes,
  Check,
  FolderOpen,
  History,
  Loader2,
  Play,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import {
  App,
  AppInputField,
  AppRun,
  AppVersionMeta,
  archiveApp,
  createApp,
  listActiveApps,
  listArchivedApps,
  listAppRuns,
  listAppVersions,
  openAppOutput,
  parseInputSchema,
  restoreAppVersion,
  runApp,
  unarchiveApp,
} from "../../lib/apps";
import { useConfirm } from "../../lib/useConfirm";
import { formatMonthDayTime, relativeTime } from "../../format";
// 程序图标砖与时间网格计划块共用一套 --plan-* 色板 + 稳定哈希：
// 同一 id 永远同色，重启/重排不变色
import { planColorVar } from "../TaskTimeline/week";
import { Modal } from "../../ui/Modal";
import { EmptyState } from "../EmptyState";
import {
  formatCommandError,
  handleCommandError,
} from "../../lib/errorHandler";

/** planColorVar 收 tag 数组取首项哈希；程序砖以 id 为种子 */
const tileColor = (id: string) => planColorVar([id]);

export function AppsPage() {
  const confirm = useConfirm();
  const [apps, setApps] = useState<App[]>([]);
  const [archived, setArchived] = useState<App[]>([]);
  /** 已归档视图：归档是程序页的用户动作，没有恢复入口就是单向门 */
  const [showArchived, setShowArchived] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [lastRuns, setLastRuns] = useState<Record<string, AppRun | null>>({});
  const [createOpen, setCreateOpen] = useState(false);
  const [running, setRunning] = useState<string | null>(null);
  const [lastResult, setLastResult] = useState<{
    appId: string;
    appName: string;
    durationMs: number;
    summary: string | null;
  } | null>(null);

  // 每张卡一条「上次运行」状态线：limit=1 轻查询；单卡失败退「尚未运行」不阻塞整页
  const loadLastRun = useCallback((appId: string) => {
    listAppRuns(appId, 1)
      .then((runs) =>
        setLastRuns((prev) => ({ ...prev, [appId]: runs[0] ?? null })),
      )
      .catch(() => setLastRuns((prev) => ({ ...prev, [appId]: null })));
  }, []);

  const reload = useCallback(() => {
    Promise.all([listActiveApps(), listArchivedApps()])
      .then(([list, archivedList]) => {
        setApps(list);
        setArchived(archivedList);
        setLoaded(true);
        // 已归档视图不查上次运行：那行程序本来就不跑了，历史只在恢复后再看
        if (!showArchived) list.forEach((a) => loadLastRun(a.id));
      })
      .catch((e) => handleCommandError(e, "加载程序列表", { silent: true }));
  }, [loadLastRun, showArchived]);
  useEffect(() => {
    reload();
  }, [reload]);

  const handleArchive = async (a: App) => {
    const ok = await confirm({
      title: "归档程序",
      message: `把「${a.name}」移出程序页？脚本、数据与运行历史都保留，可在「已归档」里恢复。`,
      okLabel: "归档",
    });
    if (!ok) return;
    try {
      await archiveApp(a.id);
      reload();
    } catch (e) {
      handleCommandError(e, "归档程序");
    }
  };

  const handleUnarchive = async (a: App) => {
    try {
      await unarchiveApp(a.id);
      reload();
    } catch (e) {
      handleCommandError(e, "恢复程序");
    }
  };

  const handleRun = (a: App) => {
    setRunning(a.id);
    runApp(a.id, null, undefined)
      .then((r) => {
        setLastResult({
          appId: a.id,
          appName: a.name,
          durationMs: r.durationMs,
          summary: r.summary,
        });
        loadLastRun(a.id);
      })
      .catch((e) => {
        handleCommandError(e, `运行 ${a.name}`);
      })
      .finally(() => setRunning(null));
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <h2 className="text-base font-semibold text-[var(--t1)]">程序</h2>
        {loaded && apps.length > 0 && (
          <span className="nm-tag text-[10px] tabular-nums">
            {apps.length} 个
          </span>
        )}
        <span className="hidden text-xs text-[var(--t5)] sm:inline">
          运行不连大模型，零成本
        </span>
        <div className="flex-1" />
        {archived.length > 0 && (
          <button
            className={`nm-btn flex h-8 shrink-0 items-center gap-1.5 whitespace-nowrap px-2.5 text-xs ${
              showArchived ? "text-[var(--t1)]" : "text-[var(--t4)]"
            }`}
            aria-pressed={showArchived}
            onClick={() => setShowArchived((v) => !v)}
            title="已归档的程序（可恢复）"
          >
            <Archive size={13} aria-hidden />
            已归档 {archived.length}
          </button>
        )}
        <button
          className="nm-btn-brand flex h-8 shrink-0 items-center gap-1.5 whitespace-nowrap px-3 text-sm"
          onClick={() => setCreateOpen(true)}
        >
          <Plus size={14} aria-hidden /> 上架程序
        </button>
      </div>

      {lastResult && (
        <ResultBanner
          result={lastResult}
          onDismiss={() => setLastResult(null)}
        />
      )}

      {showArchived ? (
        archived.length === 0 ? (
          <EmptyState
            icon={<Archive size={18} aria-hidden />}
            title="没有已归档的程序"
            description="在程序页点卡上的删除按钮，程序会移到这里（脚本与数据都保留）"
          />
        ) : (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(250px,1fr))] gap-3">
            {archived.map((a) => (
              <AppCard
                key={a.id}
                app={a}
                lastRun={undefined}
                archived
                running={false}
                onRestore={() => handleUnarchive(a)}
                onChanged={reload}
              />
            ))}
          </div>
        )
      ) : loaded && apps.length === 0 && !createOpen ? (
        <EmptyState
          icon={<Boxes size={18} aria-hidden />}
          title="还没有程序"
          description="把 app.py 放进 data/apps/<id>/ 目录，点「上架程序」登记即可"
          action={{ label: "上架程序", onClick: () => setCreateOpen(true) }}
        />
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(250px,1fr))] gap-3">
          {apps.map((a) => (
            <AppCard
              key={a.id}
              app={a}
              lastRun={lastRuns[a.id]}
              running={running === a.id}
              onRun={() => handleRun(a)}
              onArchive={() => handleArchive(a)}
              onChanged={reload}
            />
          ))}
        </div>
      )}

      {createOpen && (
        <CreateDialog
          onClose={() => setCreateOpen(false)}
          onCreated={() => {
            setCreateOpen(false);
            reload();
          }}
        />
      )}
    </div>
  );
}

function AppCard({
  app,
  lastRun,
  running,
  archived,
  onRun,
  onArchive,
  onRestore,
  onChanged,
}: {
  app: App;
  lastRun: AppRun | null | undefined;
  running: boolean;
  /** 已归档视图：不显示运行/删除，只给恢复 */
  archived?: boolean;
  onRun?: () => void;
  /** 删除 = 归档（脚本与数据保留，可在「已归档」里恢复） */
  onArchive?: () => void;
  onRestore?: () => void;
  /** 卡内发生了内容变更（如版本回滚）：通知页级重载列表 */
  onChanged: () => void;
}) {
  const [historyOpen, setHistoryOpen] = useState(false);
  const inputKind = parseInputSchema(app.inputSchema)?.kind ?? "folder";
  return (
    <div className="nm-card nm-card-hover flex flex-col gap-2.5 p-4">
      <div className="flex items-start gap-3">
        <span
          aria-hidden
          className="app-tile flex h-11 w-11 shrink-0 items-center justify-center rounded-[var(--r-md)] text-xl"
          style={{ "--c": tileColor(app.id) } as React.CSSProperties}
        >
          {app.icon || "📦"}
        </span>
        <div className="min-w-0 flex-1 pt-0.5">
          <p className="truncate text-sm font-medium text-[var(--t1)]">
            {app.name}
          </p>
          <p className="mt-1 line-clamp-2 text-xs leading-relaxed text-[var(--t4)]">
            {app.description || "（无说明）"}
          </p>
        </div>
      </div>

      <div className="flex items-center gap-1.5 text-[10px] text-[var(--t5)]">
        <LastRunLine run={lastRun} />
        <span className="flex-1" />
        {app.status === "draft" && <DraftBadge />}
        <span className="nm-tag text-[10px]">
          {inputKind === "form" ? "表单" : "文件夹"}
        </span>
      </div>

      <div className="mt-auto flex items-center gap-1">
        {archived ? (
          <button
            className="nm-btn flex h-7 items-center gap-1.5 rounded-[var(--r-sm)] px-3 text-xs font-medium text-[var(--t1)]"
            onClick={onRestore}
            title="恢复回程序页（草稿态，可试跑）"
          >
            <ArchiveRestore size={12} aria-hidden />
            恢复
          </button>
        ) : (
          <button
            className="nm-btn flex h-7 items-center gap-1.5 rounded-[var(--r-sm)] px-3 text-xs font-medium text-[var(--t1)]"
            onClick={onRun}
            disabled={running}
            title="按 data_dir/apps/<id>/ 子进程跑脚本，零 LLM"
          >
            {running ? (
              <Loader2 size={12} aria-hidden className="animate-spin" />
            ) : (
              <Play size={12} aria-hidden />
            )}
            {running ? "运行中…" : "运行"}
          </button>
        )}
        <div className="flex-1" />
        <button
          className={`nm-icon-btn h-7 w-7 ${
            historyOpen ? "text-[var(--t1)]" : ""
          }`}
          title="运行历史"
          aria-label="运行历史"
          aria-expanded={historyOpen}
          onClick={() => setHistoryOpen((v) => !v)}
        >
          <History size={13} aria-hidden />
        </button>
        {!archived && (
          <button
            className="nm-icon-btn h-7 w-7 text-[var(--t5)] hover:text-[var(--danger)]"
            title="删除程序（移入已归档，可恢复）"
            aria-label={`删除程序：${app.name}`}
            onClick={onArchive}
          >
            <Trash2 size={13} aria-hidden />
          </button>
        )}
        <button
          className="nm-icon-btn h-7 w-7"
          title="在系统文件管理器中打开 output/"
          aria-label="打开产物目录"
          onClick={() => {
            openAppOutput(app.id).catch((e) =>
              handleCommandError(e, "打开产物目录"),
            );
          }}
        >
          <FolderOpen size={13} aria-hidden />
        </button>
      </div>

      {historyOpen && <HistoryPanel appId={app.id} onChanged={onChanged} />}
    </div>
  );
}

function LastRunLine({ run }: { run: AppRun | null | undefined }) {
  if (run === undefined) return null;
  const dot = (cls: string) => (
    <span aria-hidden className={`h-1.5 w-1.5 shrink-0 rounded-full ${cls}`} />
  );
  if (run === null) {
    return (
      <span className="inline-flex items-center gap-1.5">
        {dot("bg-[var(--t6)]")}
        尚未运行
      </span>
    );
  }
  const ok = run.status === "ok";
  return (
    <span className="inline-flex items-center gap-1.5">
      {dot(ok ? "bg-[var(--success)]" : "bg-[var(--danger)]")}
      {ok ? "上次成功" : "上次失败"} · {relativeTime(run.startedAt)}
    </span>
  );
}

/** 草稿徽章（M2）：AI 创建的草稿程序，可试跑、满意后由 AI 置 enabled 上架 */
function DraftBadge() {
  return (
    <span
      className="nm-tag text-[10px]"
      title="AI 开发中的草稿程序：可试跑，满意后让 AI 上架"
      style={{
        color: "var(--plan-amber)",
        borderColor: "color-mix(in srgb, var(--plan-amber) 40%, transparent)",
        background: "color-mix(in srgb, var(--plan-amber) 10%, transparent)",
      }}
    >
      草稿
    </span>
  );
}

function HistoryPanel({
  appId,
  onChanged,
}: {
  appId: string;
  onChanged: () => void;
}) {
  const [tab, setTab] = useState<"runs" | "versions">("runs");
  return (
    <div className="nm-inset flex flex-col gap-1.5 rounded-[var(--r-sm)] p-2.5">
      <div className="flex items-center gap-0.5">
        <TabChip active={tab === "runs"} onClick={() => setTab("runs")}>
          运行
        </TabChip>
        <TabChip active={tab === "versions"} onClick={() => setTab("versions")}>
          版本
        </TabChip>
      </div>
      {tab === "runs" ? (
        <RunsList appId={appId} />
      ) : (
        <VersionsList appId={appId} onChanged={onChanged} />
      )}
    </div>
  );
}

function TabChip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      className={`rounded-[var(--r-sm)] px-2 py-0.5 text-[10px] transition-colors ${
        active
          ? "bg-[var(--hover-bg)] font-medium text-[var(--t1)]"
          : "text-[var(--t5)] hover:text-[var(--t3)]"
      }`}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

function RunsList({ appId }: { appId: string }) {
  const [runs, setRuns] = useState<AppRun[] | null>(null);
  useEffect(() => {
    listAppRuns(appId, 10)
      .then(setRuns)
      .catch(() => setRuns([]));
  }, [appId]);
  if (runs === null) {
    return <p className="text-[10px] text-[var(--t5)]">加载中…</p>;
  }
  if (runs.length === 0) {
    return <p className="text-[10px] text-[var(--t5)]">还没有运行记录</p>;
  }
  return (
    <div className="flex flex-col gap-1.5">
      {runs.map((r) => (
        <div
          key={r.id}
          className="flex items-center gap-2 text-[10px] text-[var(--t4)]"
        >
          <span
            aria-hidden
            className={`h-1.5 w-1.5 shrink-0 rounded-full ${
              r.status === "ok" ? "bg-[var(--success)]" : "bg-[var(--danger)]"
            }`}
          />
          <span className="shrink-0 tabular-nums">
            {formatMonthDayTime(r.startedAt)}
          </span>
          {r.durationMs != null && (
            <span className="shrink-0 tabular-nums text-[var(--t5)]">
              {(r.durationMs / 1000).toFixed(1)}s
            </span>
          )}
          {r.summary && (
            <span className="min-w-0 flex-1 truncate" title={r.summary}>
              {r.summary}
            </span>
          )}
        </div>
      ))}
    </div>
  );
}

const SOURCE_LABELS: Record<string, string> = {
  create: "创建",
  update: "更新",
  restore: "回滚",
};

function VersionsList({
  appId,
  onChanged,
}: {
  appId: string;
  onChanged: () => void;
}) {
  const [versions, setVersions] = useState<AppVersionMeta[] | null>(null);
  const [busy, setBusy] = useState(false);
  const confirm = useConfirm();
  const reload = useCallback(() => {
    listAppVersions(appId)
      .then(setVersions)
      .catch(() => setVersions([]));
  }, [appId]);
  useEffect(() => {
    reload();
  }, [reload]);
  if (versions === null) {
    return <p className="text-[10px] text-[var(--t5)]">加载中…</p>;
  }
  if (versions.length === 0) {
    return <p className="text-[10px] text-[var(--t5)]">还没有版本快照</p>;
  }
  return (
    <div className="flex flex-col gap-1">
      {versions.map((v, i) => (
        <div
          key={v.id}
          className="flex items-center gap-2 text-[10px] text-[var(--t4)]"
        >
          <span className="shrink-0 font-medium tabular-nums text-[var(--t3)]">
            v{v.version}
          </span>
          <span className="shrink-0 tabular-nums">
            {formatMonthDayTime(v.createdAt)}
          </span>
          <span className="shrink-0">{SOURCE_LABELS[v.source] ?? v.source}</span>
          <span className="min-w-0 flex-1 truncate" title={v.name}>
            {v.name}
          </span>
          {/* 最新版（列表首位）就是当前内容，不提供回滚 */}
          {i > 0 && (
            <button
              type="button"
              className="nm-btn shrink-0 px-1.5 py-0.5 text-[10px] text-[var(--t3)] disabled:opacity-55"
              disabled={busy}
              onClick={async () => {
                const ok = await confirm({
                  title: "回滚版本",
                  message: `回滚到 v${v.version}？当前内容会先存为新版本，可再次回滚撤销。`,
                  okLabel: "回滚",
                });
                if (!ok) return;
                setBusy(true);
                try {
                  await restoreAppVersion(appId, v.version);
                  reload();
                  onChanged();
                } catch (e) {
                  handleCommandError(e, "回滚版本");
                } finally {
                  setBusy(false);
                }
              }}
            >
              {busy ? "回滚中…" : "回滚"}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

function ResultBanner({
  result,
  onDismiss,
}: {
  result: {
    appId: string;
    appName: string;
    durationMs: number;
    summary: string | null;
  };
  onDismiss: () => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-2 rounded-[var(--r-md)] border border-[color-mix(in_srgb,var(--success)_35%,transparent)] bg-[color-mix(in_srgb,var(--success)_9%,var(--surface))] px-3 py-2 text-xs">
      <Check size={14} aria-hidden className="shrink-0 text-[var(--success)]" />
      <span className="shrink-0 font-medium text-[var(--t1)]">
        「{result.appName}」运行完成
      </span>
      <span className="shrink-0 tabular-nums text-[var(--t4)]">
        {(result.durationMs / 1000).toFixed(1)}s
      </span>
      {result.summary ? (
        <span
          className="min-w-0 flex-1 truncate text-[var(--t4)]"
          title={result.summary}
        >
          {result.summary}
        </span>
      ) : (
        <span className="flex-1" />
      )}
      <button
        type="button"
        className="shrink-0 text-[var(--t3)] underline decoration-[var(--edge-strong)] underline-offset-2 hover:text-[var(--t1)]"
        onClick={() => {
          openAppOutput(result.appId).catch((e) =>
            handleCommandError(e, "打开产物目录"),
          );
        }}
      >
        打开产物目录
      </button>
      <button
        type="button"
        className="nm-icon-btn h-6 w-6 shrink-0"
        aria-label="关闭"
        onClick={onDismiss}
      >
        <X size={13} aria-hidden />
      </button>
    </div>
  );
}

function CreateDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: () => void;
}) {
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [icon, setIcon] = useState("📦");
  const [inputKind, setInputKind] = useState<"folder" | "form">("folder");
  const [fields, setFields] = useState<AppInputField[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const submit = () => {
    setErr(null);
    if (!id.trim() || !name.trim()) {
      setErr("id 与名称不能为空");
      return;
    }
    setBusy(true);
    const inputSchema = JSON.stringify(
      inputKind === "folder"
        ? { kind: "folder" }
        : { kind: "form", fields },
    );
    createApp({
      id: id.trim(),
      name: name.trim(),
      description,
      icon,
      inputSchema,
      scriptPath: "app.py",
      manual: "",
      status: "enabled",
    })
      .then(onCreated)
      .catch((e) => {
        setErr(formatCommandError(e));
        setBusy(false);
      });
  };

  return (
    <Modal
      closeOnEsc
      onClose={onClose}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6"
    >
      <div
        className="nm-card flex w-full max-w-md flex-col gap-3 p-5"
        role="dialog"
        aria-modal="true"
        aria-label="上架程序"
      >
        <div className="flex items-center">
          <p className="text-sm font-semibold text-[var(--t1)]">上架程序</p>
          <span className="flex-1" />
          <button
            type="button"
            className="nm-icon-btn h-6 w-6"
            aria-label="关闭"
            onClick={onClose}
          >
            <X size={14} aria-hidden />
          </button>
        </div>
        <p className="-mt-1.5 text-[11px] leading-relaxed text-[var(--t5)]">
          脚本须已放在 data/apps/&lt;id&gt;/ 下；上架只登记元数据并检查脚本可读
        </p>
        <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
          <Field label={"ID（路径分量，不能含 / 或 ..）"}>
            <input
              value={id}
              onChange={(e) => setId(e.target.value)}
              placeholder="wage-match"
              className="nm-input w-full px-2 py-1.5 font-mono text-xs"
            />
          </Field>
          <Field label="名称">
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="工资单匹配"
              className="nm-input w-full px-2 py-1.5 text-xs"
            />
          </Field>
          <Field label="图标（emoji，1–4 字符）">
            <div className="flex items-center gap-2">
              <span
                aria-hidden
                className="app-tile flex h-8 w-8 shrink-0 items-center justify-center rounded-[var(--r-sm)] text-base"
                style={
                  { "--c": tileColor(id.trim() || "preview") } as React.CSSProperties
                }
              >
                {icon || "📦"}
              </span>
              <input
                value={icon}
                onChange={(e) => setIcon(e.target.value)}
                maxLength={4}
                className="nm-input w-16 px-2 py-1.5 text-sm"
              />
            </div>
          </Field>
          <Field label="输入模式">
            <div className="flex gap-1">
              <button
                type="button"
                className={`flex-1 rounded-[var(--r-sm)] px-2 py-1.5 text-xs ${
                  inputKind === "folder"
                    ? "nm-inset font-medium text-[var(--t1)]"
                    : "nm-outset text-[var(--t3)]"
                }`}
                onClick={() => setInputKind("folder")}
              >
                文件夹
              </button>
              <button
                type="button"
                className={`flex-1 rounded-[var(--r-sm)] px-2 py-1.5 text-xs ${
                  inputKind === "form"
                    ? "nm-inset font-medium text-[var(--t1)]"
                    : "nm-outset text-[var(--t3)]"
                }`}
                onClick={() => {
                  setInputKind("form");
                  if (fields.length === 0) {
                    setFields([{ key: "k1", label: "字段1", type: "text" }]);
                  }
                }}
              >
                表单
              </button>
            </div>
          </Field>
        </div>
        <p className="-mt-1 text-[11px] text-[var(--t5)]">
          {inputKind === "folder"
            ? "约定 data/ input/ output/ 子目录，运行不带参数"
            : "运行时手填字段（M2 完整实现，M1 仅骨架）"}
        </p>
        <Field label="说明">
          <textarea
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            rows={2}
            className="nm-input w-full px-2 py-1.5 text-xs"
          />
        </Field>
        {inputKind === "form" && (
          <div className="flex flex-col gap-1">
            {fields.map((f) => (
              <div
                key={f.key}
                className="flex items-center gap-1.5 text-[11px] text-[var(--t4)]"
              >
                <code>{f.key}</code> · {f.label} · {f.type}
              </div>
            ))}
          </div>
        )}
        {err && <p className="text-xs text-[var(--danger)]">{err}</p>}
        <div className="flex justify-end gap-2 pt-1">
          <button
            type="button"
            className="nm-btn px-3 py-1.5 text-xs text-[var(--t4)]"
            onClick={onClose}
            disabled={busy}
          >
            取消
          </button>
          <button
            type="button"
            className="nm-btn-brand px-4 py-1.5 text-xs font-medium"
            onClick={submit}
            disabled={busy}
          >
            {busy ? "上架中…" : "确认上架"}
          </button>
        </div>
      </div>
    </Modal>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] text-[var(--t5)]">{label}</span>
      {children}
    </label>
  );
}
