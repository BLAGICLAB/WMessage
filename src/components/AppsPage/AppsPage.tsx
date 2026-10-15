import { useCallback, useEffect, useState } from "react";
import { Plus, Play, History, FileText } from "lucide-react";
import {
  App,
  AppInputField,
  AppRun,
  createApp,
  listAppRuns,
  listEnabledApps,
  parseInputSchema,
  runApp,
} from "../../lib/apps";
import { EmptyState } from "../EmptyState";
import { handleCommandError } from "../../lib/errorHandler";

export function AppsPage() {
  const [apps, setApps] = useState<App[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [running, setRunning] = useState<string | null>(null);
  const [lastResult, setLastResult] = useState<{
    appId: string;
    appName: string;
  } | null>(null);

  const reload = useCallback(() => {
    listEnabledApps()
      .then((list) => {
        setApps(list);
        setLoaded(true);
      })
      .catch((e) => handleCommandError(e, "加载程序列表", { silent: true }));
  }, []);
  useEffect(() => {
    reload();
  }, [reload]);

  const handleRun = (a: App) => {
    setRunning(a.id);
    runApp(a.id, null, undefined)
      .then(() => {
        setLastResult({ appId: a.id, appName: a.name });
      })
      .catch((e) => {
        handleCommandError(e, `运行 ${a.name}`);
      })
      .finally(() => setRunning(null));
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex shrink-0 flex-wrap items-center gap-2 pb-1">
        <h2 className="text-base font-semibold text-[var(--t1)]">程序</h2>
        <div className="flex-1" />
        <button
          className="flex h-8 items-center gap-1.5 whitespace-nowrap rounded-[var(--r-sm)] px-3 text-sm text-[var(--t3)] nm-outset"
          onClick={() => setCreateOpen(true)}
        >
          <Plus size={14} aria-hidden /> 上架程序
        </button>
      </div>

      {loaded && apps.length === 0 && !createOpen ? (
        <EmptyState
          icon={<FileText size={18} aria-hidden />}
          title="还没有程序"
          description="点「上架程序」登记一个 app.py；运行态不连大模型，零成本"
        />
      ) : (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {apps.map((a) => (
            <AppCard
              key={a.id}
              app={a}
              running={running === a.id}
              onRun={() => handleRun(a)}
            />
          ))}
        </div>
      )}

      {lastResult && <ResultBanner result={lastResult} onDismiss={() => setLastResult(null)} />}

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

function AppCard({ app, running, onRun }: { app: App; running: boolean; onRun: () => void }) {
  const [historyOpen, setHistoryOpen] = useState(false);
  return (
    <div className="nm-card flex flex-col gap-2 p-4">
      <div className="flex items-start gap-2">
        <span className="text-2xl leading-none" aria-hidden>
          {app.icon || "📦"}
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium text-[var(--t1)]">
            {app.name}
          </p>
          <p className="mt-0.5 line-clamp-2 text-xs text-[var(--t4)]">
            {app.description || "（无说明）"}
          </p>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        <button
          className="nm-btn rounded-lg px-3 py-1 text-xs text-[var(--t3)] disabled:opacity-50"
          onClick={onRun}
          disabled={running}
          title="按 data_dir/apps/<id>/ 子进程跑脚本，零 LLM"
        >
          <Play size={11} aria-hidden className="mr-1 inline" />
          {running ? "运行中…" : "运行"}
        </button>
        <button
          className="nm-btn rounded-lg px-3 py-1 text-xs text-[var(--t3)]"
          onClick={() => setHistoryOpen((v) => !v)}
        >
          <History size={11} aria-hidden className="mr-1 inline" />
          {historyOpen ? "收起历史" : "历史"}
        </button>
        <span className="ml-auto text-[10px] text-[var(--t5)]">
          {app.scriptPath}
        </span>
      </div>
      {historyOpen && <HistoryPanel appId={app.id} />}
    </div>
  );
}

function HistoryPanel({ appId }: { appId: string }) {
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
    <ul className="mt-1 space-y-1 border-t border-[var(--edge)] pt-2">
      {runs.map((r) => (
        <li
          key={r.id}
          className="flex items-center gap-2 text-[10px] text-[var(--t4)]"
        >
          <span
            className={`shrink-0 nm-inset px-1.5 py-0.5 ${
              r.status === "ok" ? "text-[var(--success)]" : "text-[var(--danger)]"
            }`}
          >
            {r.status === "ok" ? "成功" : "失败"}
          </span>
          <span className="shrink-0">
            {new Date(r.startedAt).toLocaleString()}
          </span>
          {r.durationMs != null && (
            <span className="shrink-0 tabular-nums">
              {(r.durationMs / 1000).toFixed(1)}s
            </span>
          )}
          {r.summary && (
            <span className="min-w-0 flex-1 truncate" title={r.summary}>
              {r.summary}
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

function ResultBanner({
  result,
  onDismiss,
}: {
  result: { appId: string; appName: string };
  onDismiss: () => void;
}) {
  return (
    <div className="nm-inset flex items-center gap-2 rounded-lg px-3 py-2 text-xs text-[var(--t3)]">
      <span className="shrink-0 text-[var(--success)]">✓</span>
      <span className="flex-1 truncate">
        「{result.appName}」运行完成 — 详情见通知
      </span>
      <button
        type="button"
        className="text-[var(--t5)] hover:text-[var(--t2)]"
        onClick={onDismiss}
      >
        ✕
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
        setErr(String(e));
        setBusy(false);
      });
  };

  return (
    <div className="nm-card flex flex-col gap-3 p-4">
      <p className="text-sm font-medium text-[var(--t2)]">上架新程序</p>
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        <Field label="ID（apps/<id>/，路径分量，不能含 / 或 ..）">
          <input
            value={id}
            onChange={(e) => setId(e.target.value)}
            placeholder="wage-match"
            className="nm-inset w-full rounded-[var(--r-sm)] px-2 py-1 text-xs"
          />
        </Field>
        <Field label="名称">
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="工资单匹配"
            className="nm-inset w-full rounded-[var(--r-sm)] px-2 py-1 text-xs"
          />
        </Field>
        <Field label="图标（emoji，1–4 字符）">
          <input
            value={icon}
            onChange={(e) => setIcon(e.target.value)}
            maxLength={4}
            className="nm-inset w-20 rounded-[var(--r-sm)] px-2 py-1 text-xs"
          />
        </Field>
        <Field label="输入模式">
          <select
            value={inputKind}
            onChange={(e) => {
              const v = e.target.value as "folder" | "form";
              setInputKind(v);
              if (v === "form" && fields.length === 0) {
                setFields([{ key: "k1", label: "字段1", type: "text" }]);
              }
            }}
            className="nm-inset w-full rounded-[var(--r-sm)] px-2 py-1 text-xs"
          >
            <option value="folder">文件夹模式（data/ input/ output/）</option>
            <option value="form">表单模式（手填字段）</option>
          </select>
        </Field>
      </div>
      <Field label="说明">
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          rows={2}
          className="nm-inset w-full rounded-[var(--r-sm)] px-2 py-1 text-xs"
        />
      </Field>
      {inputKind === "form" && (
        <div className="flex flex-col gap-1">
          <p className="text-[11px] text-[var(--t5)]">表单字段（M2 完整实现，M1 仅骨架）</p>
          {fields.map((f, i) => (
            <div key={i} className="flex items-center gap-1.5 text-[11px] text-[var(--t4)]">
              <code>{f.key}</code> · {f.label} · {f.type}
            </div>
          ))}
        </div>
      )}
      {err && <p className="text-xs text-[var(--danger)]">{err}</p>}
      <div className="flex justify-end gap-2 pt-1">
        <button
          type="button"
          className="nm-btn rounded-lg px-3 py-1 text-xs text-[var(--t4)]"
          onClick={onClose}
          disabled={busy}
        >
          取消
        </button>
        <button
          type="button"
          className="nm-btn rounded-lg px-3 py-1 text-xs text-[var(--t1)] disabled:opacity-50"
          onClick={submit}
          disabled={busy}
        >
          {busy ? "上架中…" : "确认上架"}
        </button>
      </div>
    </div>
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

// 静默占位：M2 接入表单模式后这条导入会被使用
void parseInputSchema;
