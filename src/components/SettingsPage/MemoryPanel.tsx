// SettingsPage 子模块：记忆库管理面板（U14）。
// 列表 / 搜索（后端混合检索，纯读不刷访问计数）/ 类型筛选 /
// 编辑（内容 + 重要度 + 类型；tags 与来源不可改）/ 删除；
// 统计行 + 嵌入引擎状态横幅（降级时显眼提示，界面仍可用 = 关键词模式）。
// 命令：mem_list / mem_update / mem_delete / mem_stats（后端薄层，复用 store/rank）。

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Brain, Download, Info, Pencil, RefreshCw, Trash2, Upload } from "lucide-react";
import { formatCommandError } from "../../lib/errorHandler";
import { IconButton } from "../../ui/IconButton";
import { EmptyState } from "../EmptyState";

export type MemItemView = {
  id: string;
  kind: string;
  content: string;
  tags: string[];
  importance: number;
  source: string;
  createdAt: number;
  updatedAt: number;
  accessCount: number;
  lastAccessedAt: number | null;
  hasEmbedding: boolean;
};

export type MemStats = {
  total: number;
  byKind: [string, number][];
  bySource: [string, number][];
  withEmbedding: number;
  capacity: number;
  embedOk: boolean;
  embedError: string | null;
};

/** kind → 中文徽标（与注入块的 kind 英文原样契约对齐） */
const KIND_LABELS: Record<string, string> = {
  profile: "画像",
  preference: "偏好",
  fact: "事实",
  event: "事件",
  summary: "摘要",
  reflection: "反思",
  lesson: "教训",
};
const KIND_FILTERS = [
  "all",
  "profile",
  "preference",
  "fact",
  "lesson",
  "summary",
  "reflection",
  "event",
] as const;

/** source → 中文（后端读取侧已把历史脏值 'user' 归一为 user_stated） */
const SOURCE_LABELS: Record<string, string> = {
  user_stated: "用户",
  model_inferred: "推断",
  system: "系统",
};

function fmtTime(ms: number): string {
  return new Date(ms).toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function MemoryPanel() {
  const [items, setItems] = useState<MemItemView[] | null>(null);
  const [stats, setStats] = useState<MemStats | null>(null);
  const [loadError, setLoadError] = useState("");
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [kindFilter, setKindFilter] = useState<string>("all");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editContent, setEditContent] = useState("");
  const [editImportance, setEditImportance] = useState(3);
  const [editKind, setEditKind] = useState("fact");
  const [busy, setBusy] = useState(false);
  const [rowError, setRowError] = useState("");
  /** 导出/导入结果提示（4s 自动清除，同 consolidateMsg 的 toast 模式） */
  const [ioMsg, setIoMsg] = useState("");
  const ioMsgTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 设置提示并重置自动清除计时器（重设前清旧，卸载时清尾） */
  const showIoMsg = (msg: string) => {
    if (ioMsgTimer.current) clearTimeout(ioMsgTimer.current);
    ioMsgTimer.current = setTimeout(() => setIoMsg(""), 4000);
    setIoMsg(msg);
  };
  useEffect(
    () => () => {
      if (ioMsgTimer.current) clearTimeout(ioMsgTimer.current);
    },
    [],
  );

  /** 导出记忆：plugin-dialog save 取路径 → 后端写 JSON（含向量，跨机不丢语义检索）。
   *  对话框取消不清上一条提示（清提示时机在拿到 path 之后） */
  const exportMemories = async () => {
    if (busy) return;
    try {
      const path = await save({
        defaultPath: `wmessage-memories-${new Date()
          .toISOString()
          .slice(0, 10)
          .replace(/-/g, "")}.json`,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path) return;
      setBusy(true);
      const f = await invoke<{ count: number }>("mem_export", { path });
      showIoMsg(`已导出 ${f.count} 条记忆`);
    } catch (e) {
      showIoMsg(`导出失败：${formatCommandError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  /** 导入记忆：plugin-dialog open 取路径 → 后端走既有语义去重只增不删 → 刷新列表 */
  const importMemories = async () => {
    if (busy) return;
    try {
      // open(multiple:false) 返回 string | null
      const path = await open({
        multiple: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (typeof path !== "string" || !path) return;
      setBusy(true);
      const r = await invoke<{ inserted: number; merged: number; skipped: number }>(
        "mem_import",
        { path },
      );
      showIoMsg(`导入完成：新增 ${r.inserted} 条、合并 ${r.merged} 条、跳过 ${r.skipped} 条`);
      await reload(debouncedQuery, kindFilter);
    } catch (e) {
      showIoMsg(`导入失败：${formatCommandError(e)}`);
    } finally {
      setBusy(false);
    }
  };

  // 搜索防抖 300ms（输入不中断，停顿后才打后端）
  useEffect(() => {
    const t = setTimeout(() => setDebouncedQuery(query.trim()), 300);
    return () => clearTimeout(t);
  }, [query]);

  /** 竞态令牌：慢的旧响应（查询 A）不得覆盖新的（查询 B） */
  const reqIdRef = useRef(0);

  const reload = useCallback(async (q: string, kind: string) => {
    const reqId = ++reqIdRef.current;
    // allSettled：统计失败（横幅数据）不拖垮列表刷新
    const [listRes, stRes] = await Promise.allSettled([
      invoke<MemItemView[]>("mem_list", {
        query: q || null,
        kind: kind === "all" ? null : kind,
      }),
      invoke<MemStats | null>("mem_stats"),
    ]);
    if (reqId !== reqIdRef.current) return; // 已有更新请求，丢弃过期结果
    if (listRes.status === "fulfilled") {
      const list = listRes.value;
      setItems(Array.isArray(list) ? list : []);
      setLoadError("");
    } else {
      setLoadError(formatCommandError(listRes.reason));
    }
    if (stRes.status === "fulfilled") {
      const st = stRes.value;
      if (st && typeof st === "object" && "total" in st) setStats(st);
    }
    setRowError("");
  }, []);

  useEffect(() => {
    reload(debouncedQuery, kindFilter);
  }, [debouncedQuery, kindFilter, reload]);

  const startEdit = (m: MemItemView) => {
    setEditingId(m.id);
    setEditContent(m.content);
    setEditImportance(m.importance);
    setEditKind(m.kind);
    setRowError("");
  };

  const saveEdit = async () => {
    if (!editingId) return;
    // 预检：空内容免一次后端往返（后端同样校验，双保险）
    if (!editContent.trim()) {
      setRowError("记忆内容不能为空（要删除请用删除按钮）");
      return;
    }
    setBusy(true);
    setRowError("");
    try {
      await invoke("mem_update", {
        id: editingId,
        content: editContent,
        importance: editImportance,
        kind: editKind,
      });
      setEditingId(null);
      await reload(debouncedQuery, kindFilter);
    } catch (e) {
      setRowError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  const removeItem = async (m: MemItemView) => {
    // evo: 前缀条目来自自进化链路（提案回滚的教训），删除影响回滚后注入——确认文案点名
    const fromEvolution = (m.tags?.[0] ?? "").startsWith("evo:");
    const ok = window.confirm(
      fromEvolution
        ? "该条目来自自进化链路，删除后相关提案回滚时不再注入该教训。确定删除？"
        : `删除这条记忆？\n\n${m.content.slice(0, 80)}`,
    );
    if (!ok) return;
    setBusy(true);
    setRowError("");
    try {
      await invoke("mem_delete", { id: m.id });
      await reload(debouncedQuery, kindFilter);
    } catch (e) {
      setRowError(formatCommandError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="nm-card p-5">
      <div className="space-y-2">
        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            <p className="text-xs font-medium text-[var(--t4)]">记忆库</p>
            {/* 统计行：总数/容量 + 向量覆盖（覆盖低 = 部分条目靠关键词模式检索） */}
            <p className="mt-1 text-[11px] text-[var(--t5)]">
              {stats
                ? `${stats.total}/${stats.capacity} 条 · 向量覆盖 ${stats.withEmbedding}/${stats.total}`
                : "加载中…"}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            {/* 导出/导入（U15）：向量随 JSON 带出，导入走语义去重只增不删；
                低风险的导出在左，导入紧随，刷新贴列表侧 */}
            <IconButton
              aria-label="导出记忆"
              title="导出全部记忆到 JSON 文件（含语义向量）"
              className="text-[var(--t5)] hover:text-[var(--t2)]"
              onClick={exportMemories}
              disabled={busy}
            >
              <Download size={13} aria-hidden />
            </IconButton>
            <IconButton
              aria-label="导入记忆"
              title="从 JSON 文件导入记忆（与现有记忆语义重复的会自动合并）"
              className="text-[var(--t5)] hover:text-[var(--t2)]"
              onClick={importMemories}
              disabled={busy}
            >
              <Upload size={13} aria-hidden />
            </IconButton>
            <IconButton
              aria-label="刷新记忆列表"
              title="刷新记忆列表"
              className="text-[var(--t5)] hover:text-[var(--t2)]"
              onClick={() => reload(debouncedQuery, kindFilter)}
              disabled={busy}
            >
              <RefreshCw size={13} aria-hidden />
            </IconButton>
          </div>
        </div>
        {ioMsg && <p className="text-[11px] text-[var(--t4)]">{ioMsg}</p>}
        {/* 嵌入引擎降级横幅：界面照常可用（关键词检索），但语义相似度缺位 */}
        {stats && !stats.embedOk && (
          <p
            className="flex items-start gap-1.5 text-[11px] text-[var(--t3)]"
            role="note"
            aria-label="嵌入引擎状态"
          >
            <Info size={13} className="mt-0.5 shrink-0 text-[var(--brand)]" aria-hidden />
            <span>
              语义嵌入不可用，已降级为关键词检索{stats.embedError ? `：${stats.embedError}` : ""}
            </span>
          </p>
        )}
        <div className="flex items-center gap-2">
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索记忆…"
            aria-label="搜索记忆"
            className="nm-inset min-w-0 flex-1 rounded-lg px-2.5 py-1.5 text-xs text-[var(--t3)] outline-none"
          />
        </div>
        <div className="flex flex-wrap gap-1.5">
          {KIND_FILTERS.map((k) => (
            <button
              key={k}
              type="button"
              className={`rounded-full px-2.5 py-1 text-[10px] text-[var(--t3)] ${
                kindFilter === k ? "nm-inset" : "nm-outset"
              }`}
              onClick={() => setKindFilter(k)}
            >
              {k === "all" ? "全部" : KIND_LABELS[k]}
            </button>
          ))}
        </div>
        {loadError && <p className="text-[11px] text-[var(--danger)]">{loadError}</p>}
        {rowError && <p className="text-[11px] text-[var(--danger)]">{rowError}</p>}
        {items !== null && items.length === 0 && (
          <EmptyState
            icon={<Brain size={18} aria-hidden />}
            title="还没有匹配的记忆"
            description="在聊天里让机器人「记住…」，它会把偏好和事实存进这里；记忆整理也会定期归纳。"
          />
        )}
        <div className="space-y-1">
          {(items ?? []).map((m) =>
            editingId === m.id ? (
              <div key={m.id} className="rounded-xl nm-inset px-3 py-2">
                <textarea
                  value={editContent}
                  onChange={(e) => setEditContent(e.target.value)}
                  rows={3}
                  aria-label="编辑记忆内容"
                  className="nm-inset w-full rounded-lg px-2.5 py-1.5 text-xs text-[var(--t3)] outline-none"
                />
                <div className="mt-1.5 flex items-center gap-2">
                  <label className="flex shrink-0 items-center gap-1 text-[11px] text-[var(--t3)]">
                    重要度
                    <select
                      value={editImportance}
                      onChange={(e) => setEditImportance(Number(e.target.value))}
                      aria-label="记忆重要度"
                      className="nm-inset rounded-lg px-1.5 py-1 text-[11px] text-[var(--t3)] outline-none"
                    >
                      {[1, 2, 3, 4, 5].map((n) => (
                        <option key={n} value={n}>
                          {n}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label className="flex shrink-0 items-center gap-1 text-[11px] text-[var(--t3)]">
                    类型
                    <select
                      value={editKind}
                      onChange={(e) => setEditKind(e.target.value)}
                      aria-label="记忆类型"
                      className="nm-inset rounded-lg px-1.5 py-1 text-[11px] text-[var(--t3)] outline-none"
                    >
                      {Object.entries(KIND_LABELS).map(([k, label]) => (
                        <option key={k} value={k}>
                          {label}
                        </option>
                      ))}
                    </select>
                  </label>
                  <span className="flex-1" />
                  <button
                    type="button"
                    className="nm-btn shrink-0 px-2.5 py-1 text-xs text-[var(--t2)]"
                    onClick={saveEdit}
                    disabled={busy}
                  >
                    保存
                  </button>
                  <button
                    type="button"
                    className="nm-btn shrink-0 px-2.5 py-1 text-xs text-[var(--t5)]"
                    onClick={() => setEditingId(null)}
                    disabled={busy}
                  >
                    取消
                  </button>
                </div>
              </div>
            ) : (
              <div
                key={m.id}
                className="flex items-center gap-2 rounded-xl px-3 py-2 nm-outset"
              >
                <span
                  className={`nm-tag shrink-0 text-[10px]${m.hasEmbedding ? "" : " opacity-50"}`}
                  title={m.hasEmbedding ? "语义向量可用" : "无向量（关键词模式检索）"}
                >
                  {KIND_LABELS[m.kind] ?? m.kind}
                </span>
                <span
                  className="min-w-0 flex-1 truncate text-xs text-[var(--t2)]"
                  title={m.content}
                >
                  {m.content}
                </span>
                <span
                  className="shrink-0 font-mono text-[10px] text-[var(--t5)]"
                  title={`重要度 ${m.importance}/5`}
                >
                  ★{m.importance}
                </span>
                <span className="nm-tag shrink-0 text-[10px]">
                  {SOURCE_LABELS[m.source] ?? m.source}
                </span>
                {/* 被想起次数：行内直显（= 聊天注入命中的累计次数；面板搜索不计入） */}
                {m.accessCount > 0 && (
                  <span className="shrink-0 text-[10px] text-[var(--t6)]" title="被聊天注入命中的次数">
                    想起 {m.accessCount}
                  </span>
                )}
                <span
                  className="shrink-0 text-[10px] text-[var(--t6)]"
                  title={`更新于 ${fmtTime(m.updatedAt)}`}
                >
                  {fmtTime(m.updatedAt)}
                </span>
                <IconButton
                  aria-label="编辑记忆"
                  title="编辑记忆"
                  className="shrink-0 text-[var(--t5)] hover:text-[var(--t2)]"
                  onClick={() => startEdit(m)}
                  disabled={busy}
                >
                  <Pencil size={13} aria-hidden />
                </IconButton>
                <IconButton
                  aria-label="删除记忆"
                  title="删除记忆"
                  className="shrink-0 text-[var(--t5)] hover:text-[var(--danger)]"
                  onClick={() => removeItem(m)}
                  disabled={busy}
                >
                  <Trash2 size={13} aria-hidden />
                </IconButton>
              </div>
            ),
          )}
        </div>
      </div>
    </div>
  );
}
