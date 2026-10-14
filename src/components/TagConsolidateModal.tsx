// 归档卡标签整理弹窗：portal-to-body 渲染，簇列表 + select 主标签。
// 入口在 ArchivePage 「整理标签」按钮；点关闭按钮或遮罩外部关闭。
import { useEffect, useMemo, useState } from "react";
import { Tags, X, Loader2 } from "lucide-react";
import {
  tagsConsolidateScan,
  tagsConsolidateMerge,
  type TagCluster,
} from "../lib/botConsolidate";
import { handleCommandError } from "../lib/errorHandler";
import { Modal } from "../ui/Modal";

interface Props {
  onClose: () => void;
  onMerged?: (runIds: string[]) => void;
}

interface ClusterSelection {
  /** 簇内每个成员 → 选中的主标签（默认 = 后端建议） */
  main: string;
  /** 是否纳入本次合并（默认 true；用户可单簇取消） */
  enabled: boolean;
}

export function TagConsolidateModal({ onClose, onMerged }: Props) {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [clusters, setClusters] = useState<TagCluster[]>([]);
  const [scannedTags, setScannedTags] = useState(0);
  const [submitting, setSubmitting] = useState(false);
  // 弹窗打开时一次性 scan；关闭时丢弃
  useEffect(() => {
    let alive = true;
    tagsConsolidateScan()
      .then((res) => {
        if (!alive) return;
        setClusters(res.clusters);
        setScannedTags(res.scanned_tags);
        setLoading(false);
      })
      .catch((e) => {
        if (!alive) return;
        setError(String(e));
        setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, []);

  // 每个簇的本地选中态：主标签（默认 = 后端 suggested_main）+ 是否纳入
  const [selections, setSelections] = useState<Record<number, ClusterSelection>>(
    {},
  );
  // 簇首次进列表时初始化默认选择
  useEffect(() => {
    if (clusters.length === 0) return;
    setSelections((prev) => {
      const next = { ...prev };
      for (let i = 0; i < clusters.length; i++) {
        if (next[i] === undefined) {
          next[i] = {
            main: clusters[i].suggested_main,
            enabled: true,
          };
        }
      }
      return next;
    });
  }, [clusters]);

  const selectedCount = useMemo(
    () =>
      clusters.reduce(
        (acc, _, i) => acc + (selections[i]?.enabled ? 1 : 0),
        0,
      ),
    [clusters, selections],
  );

  const close = () => {
    if (submitting) return;
    onClose();
  };

  const submit = async () => {
    setSubmitting(true);
    const runIds: string[] = [];
    try {
      for (let i = 0; i < clusters.length; i++) {
        const sel = selections[i];
        if (!sel?.enabled) continue;
        const rec = await tagsConsolidateMerge(
          sel.main,
          clusters[i].members.filter((m) => m !== sel.main),
        );
        runIds.push(rec.run_id);
      }
      onMerged?.(runIds);
      onClose();
    } catch (e) {
      handleCommandError(e, "整理标签");
      setSubmitting(false);
    }
  };

  return (
    <Modal
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/40 p-6"
      onClose={close}
      portal
    >
      <div
        className="nm-card w-full max-w-2xl max-h-[85vh] flex flex-col"
        onPointerDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 p-4 border-b border-[var(--bd)]">
          <Tags size={14} aria-hidden className="text-[var(--t3)]" />
          <span className="text-sm font-medium text-[var(--t2)]">整理归档标签</span>
          <span className="text-[11px] text-[var(--t5)] ml-2">
            {loading
              ? "扫描中…"
              : `${scannedTags} 个标签 · ${clusters.length} 个待合并簇`}
          </span>
          <button
            type="button"
            className="ml-auto p-1 text-[var(--t5)] hover:text-[var(--t2)]"
            onClick={close}
            aria-label="关闭"
          >
            <X size={14} aria-hidden />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-4">
          {loading && (
            <div className="flex items-center gap-2 text-xs text-[var(--t4)] py-4 justify-center">
              <Loader2 size={12} aria-hidden className="animate-spin" />
              扫描归档卡片…
            </div>
          )}
          {error && (
            <div className="text-xs text-[var(--danger)] py-2">{error}</div>
          )}
          {!loading && !error && clusters.length === 0 && (
            <div className="text-xs text-[var(--t5)] py-4 text-center">
              没有发现需要合并的标签簇
            </div>
          )}
          {!loading && !error && clusters.length > 0 && (
            <ul className="flex flex-col gap-3">
              {clusters.map((c, i) => {
                const sel = selections[i] ?? {
                  main: c.suggested_main,
                  enabled: true,
                };
                return (
                  <li
                    key={c.members.join("|")}
                    className={`rounded-lg p-3 border ${
                      sel.enabled
                        ? "border-[var(--bd)] bg-[var(--bg)]"
                        : "border-dashed border-[var(--bd)] bg-transparent opacity-60"
                    }`}
                  >
                    <div className="flex items-center gap-2 mb-2">
                      <input
                        type="checkbox"
                        checked={sel.enabled}
                        onChange={(e) =>
                          setSelections((s) => ({
                            ...s,
                            [i]: { ...sel, enabled: e.target.checked },
                          }))
                        }
                        aria-label={`纳入簇 ${i + 1}`}
                      />
                      <span className="text-[11px] text-[var(--t5)]">
                        簇 {i + 1} · avg sim{" "}
                        {c.avg_similarity.toFixed(2)} · 影响{" "}
                        {c.affected_task_count} 任务
                      </span>
                    </div>
                    <div className="flex flex-wrap items-center gap-1.5 mb-2">
                      {c.members.map((m) => (
                        <span
                          key={m}
                          className={`px-2 py-0.5 text-xs rounded-full ${
                            sel.main === m
                              ? "nm-inset text-[var(--t1)] font-medium"
                              : "nm-outset text-[var(--t4)]"
                          }`}
                        >
                          #{m}
                        </span>
                      ))}
                    </div>
                    <div className="flex items-center gap-2 text-[11px] text-[var(--t4)]">
                      <span>主标签：</span>
                      <select
                        value={sel.main}
                        onChange={(e) =>
                          setSelections((s) => ({
                            ...s,
                            [i]: { ...sel, main: e.target.value },
                          }))
                        }
                        className="nm-inset px-2 py-1 rounded text-xs text-[var(--t2)] outline-none flex-1 min-w-0"
                        disabled={!sel.enabled}
                      >
                        {c.members.map((m) => (
                          <option key={m} value={m}>
                            #{m}
                          </option>
                        ))}
                        <option value="__new__">+ 新建主标签…</option>
                      </select>
                      {sel.main === "__new__" && (
                        <input
                          autoFocus
                          type="text"
                          placeholder="新主标签名"
                          className="nm-inset px-2 py-1 rounded text-xs text-[var(--t2)] outline-none w-32"
                          onChange={(e) =>
                            setSelections((s) => ({
                              ...s,
                              [i]: { ...sel, main: e.target.value },
                            }))
                          }
                        />
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </div>

        <div className="flex items-center gap-2 p-4 border-t border-[var(--bd)]">
          <span className="text-[11px] text-[var(--t5)]">
            {selectedCount > 0
              ? `将合并 ${selectedCount} 个簇`
              : "未选任何簇"}
          </span>
          <button
            type="button"
            className="ml-auto nm-btn px-3 py-1.5 text-xs text-[var(--t3)]"
            onClick={close}
            disabled={submitting}
          >
            取消
          </button>
          <button
            type="button"
            className="nm-btn px-3 py-1.5 text-xs text-[var(--t2)] disabled:opacity-50"
            onClick={submit}
            disabled={loading || error !== null || selectedCount === 0 || submitting}
          >
            {submitting ? (
              <>
                <Loader2 size={11} aria-hidden className="inline animate-spin mr-1" />
                合并中…
              </>
            ) : (
              <>合并 {selectedCount} 个簇</>
            )}
          </button>
        </div>
      </div>
    </Modal>
  );
}
