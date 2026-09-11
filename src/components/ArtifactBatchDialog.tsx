import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { formatCommandError } from "../lib/errorHandler";

type BatchReady = {
  taskId: string;
  taskTitle: string;
  sessionId: string;
  origin: "manual" | "scheduled" | "batch";
  paths: string[];
};

const ORIGIN_LABEL: Record<BatchReady["origin"], string> = {
  manual: "🤖 手动执行",
  scheduled: "⏰ 定时执行",
  batch: "📦 批量执行",
};

/** 任务卡执行流程结束后的产物汇总弹窗（D4d 触发）
 *
 * 挂在挂件（聊天）窗口 WidgetApp 内——挂件进程常驻、面板始终挂载，
 * 不管挂件收起还是锁定，事件都不丢；挂件收起时由 WidgetApp 自动展开
 * 让弹窗可见。监听后端 `artifact-batch-ready` 事件（由
 * run_task_in_chat_with 收尾按 TaskExecOrigin 分流 emit）。默认全选，
 * 用户可勾选/取消，确认后调 `confirm_artifact_batch` 落 db_upsert。
 */
export function ArtifactBatchDialog() {
  const [ready, setReady] = useState<BatchReady | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    const un = listen<BatchReady>("artifact-batch-ready", (e) => {
      setReady(e.payload);
      setSelected(new Set(e.payload.paths));
      setError("");
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  if (!ready) return null;

  const toggle = (path: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const allChecked = ready.paths.every((p) => selected.has(p));
  const toggleAll = () => {
    if (allChecked) setSelected(new Set());
    else setSelected(new Set(ready.paths));
  };

  const confirm = async () => {
    setBusy(true);
    setError("");
    try {
      const paths = Array.from(selected);
      await invoke<number>("confirm_artifact_batch", {
        taskId: ready.taskId,
        paths,
      });
      setReady(null);
    } catch (e) {
      setError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const skip = () => setReady(null);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6">
      <div className="bg-white dark:bg-gray-800 rounded-lg shadow-xl max-w-2xl w-full max-h-[80vh] flex flex-col">
        <div className="px-6 py-4 border-b border-gray-200 dark:border-gray-700">
          <h2 className="text-lg font-semibold">bot 流程产物绑定</h2>
          <p className="text-sm text-gray-500 mt-1">
            任务「{ready.taskTitle}」已完成（{ORIGIN_LABEL[ready.origin]}）。
            以下是 bot 流程生成的最终产物，勾选要绑定到任务卡的文件（默认全选）。
          </p>
        </div>
        <div className="px-6 py-3 border-b border-gray-200 dark:border-gray-700 flex items-center gap-2">
          <input
            type="checkbox"
            checked={allChecked}
            onChange={toggleAll}
            disabled={busy}
          />
          <span className="text-sm">全选</span>
          <span className="text-xs text-gray-500 ml-auto">
            已选 {selected.size} / {ready.paths.length}
          </span>
        </div>
        <div className="px-6 py-4 overflow-y-auto flex-1">
          <ul className="space-y-2">
            {ready.paths.map((p) => (
              <li key={p} className="flex items-start gap-2">
                <input
                  type="checkbox"
                  checked={selected.has(p)}
                  onChange={() => toggle(p)}
                  disabled={busy}
                  className="mt-1"
                />
                <span className="text-sm break-all font-mono">{p}</span>
              </li>
            ))}
          </ul>
        </div>
        {error && (
          <div className="px-6 py-2 text-sm text-red-500 border-t border-gray-200 dark:border-gray-700">
            {error}
          </div>
        )}
        <div className="px-6 py-4 border-t border-gray-200 dark:border-gray-700 flex justify-end gap-2">
          <button
            onClick={skip}
            disabled={busy}
            className="px-4 py-2 text-sm rounded border border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-700 disabled:opacity-50"
          >
            跳过
          </button>
          <button
            onClick={confirm}
            disabled={busy || selected.size === 0}
            className="px-4 py-2 text-sm rounded bg-blue-500 text-white hover:bg-blue-600 disabled:opacity-50"
          >
            {busy ? "绑定中..." : `绑定选中 (${selected.size})`}
          </button>
        </div>
      </div>
    </div>
  );
}
