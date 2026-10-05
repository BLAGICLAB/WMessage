// 任务图谱页（任务图谱设计 §3）：全量任务（含归档、排除回收站）的 Obsidian 式
// 关系图谱——力导向布局、依赖边 + 工作流 hub、按状态/成员着色、过滤器侧栏、
// hover 邻接高亮、点选详情。看板/挂件只看本人任务；这里看整个部门。

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Waypoints } from "lucide-react";
import type { PeopleEntry, Task, Workflow } from "../../types";
import { loadPeople } from "../../storage";
import GraphCanvas, { OWNER_PALETTE } from "./GraphCanvas";
import {
  DEFAULT_FILTERS,
  SELF_OWNER,
  buildTaskGraph,
  collectOwners,
  collectTags,
  collectYears,
  type GraphColorMode,
  type GraphFilters,
} from "./graph-build";

const STATUS_LABELS: Record<string, string> = {
  todo: "待办",
  doing: "进行中",
  done: "已完成",
};

interface GraphPageProps {
  /** 全量任务（App 传 tasks 而非 visibleTasks——图谱自带过滤，需要看到所有人） */
  tasks: Task[];
  /** 详情面板「在看板打开」：App 的 jumpToTask（按任务位置切视图 + 编辑态） */
  onOpenTask: (t: Task) => void;
  /** 双击工作流 hub → 打开工作流画布 */
  onOpenWorkflow: () => void;
}

export default function GraphPage({ tasks, onOpenTask, onOpenWorkflow }: GraphPageProps) {
  const [workflows, setWorkflows] = useState<Workflow[]>([]);
  const [people, setPeople] = useState<PeopleEntry[]>([]);
  const [filters, setFilters] = useState<GraphFilters>(DEFAULT_FILTERS);
  const [colorMode, setColorMode] = useState<GraphColorMode>("status");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [hoverId, setHoverId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [showFilters, setShowFilters] = useState(true);

  useEffect(() => {
    invoke<Workflow[]>("workflow_list")
      .then(setWorkflows)
      .catch((e) => console.error("[graph] workflow_list failed", e));
    const refreshPeople = () =>
      loadPeople()
        .then(setPeople)
        .catch((e) => console.error("[graph] people_list failed", e));
    refreshPeople();
    // 导入/资料变更后成员表会新增，tasks-changed 时顺带刷新（与任务数据同频）
    const un = listen("tasks-changed", refreshPeople);
    return () => {
      un.then((f) => f());
    };
  }, []);

  const graph = useMemo(
    () => buildTaskGraph(tasks, workflows, filters),
    [tasks, workflows, filters]
  );
  const ownerChips = useMemo(() => collectOwners(tasks, people), [tasks, people]);
  const tagList = useMemo(() => collectTags(tasks), [tasks]);
  const years = useMemo(() => collectYears(tasks), [tasks]);

  // owner 着色：本人 = brand（CSS 变量在 canvas 内解析），外来成员按序取固定色盘
  const ownerColors = useMemo(() => {
    const map = new Map<string, string>();
    map.set(SELF_OWNER, "var(--brand)");
    let i = 0;
    for (const p of ownerChips) {
      if (p.isSelf) continue;
      map.set(p.id, OWNER_PALETTE[i % OWNER_PALETTE.length]);
      i += 1;
    }
    return map;
  }, [ownerChips]);

  const searchMatchIds = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return null;
    const ids = new Set<string>();
    for (const n of graph.nodes) {
      if (
        n.label.toLowerCase().includes(q) ||
        (n.tags ?? []).some((t) => t.toLowerCase().includes(q))
      ) {
        ids.add(n.id);
      }
    }
    return ids;
  }, [search, graph.nodes]);

  const selectedNode = useMemo(
    () => graph.nodes.find((n) => n.id === selectedId) ?? null,
    [graph.nodes, selectedId]
  );

  const toggleOwner = (id: string) => {
    setFilters((f) => {
      const cur = f.owners ?? ownerChips.map((o) => o.id);
      const next = cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id];
      return { ...f, owners: next.length === ownerChips.length ? null : next };
    });
  };

  const toggleTag = (tag: string) => {
    setFilters((f) => {
      const cur = f.tags ?? [];
      const next = cur.includes(tag) ? cur.filter((x) => x !== tag) : [...cur, tag];
      return { ...f, tags: next.length === 0 ? null : next };
    });
  };

  const toggleWorkflow = (id: string) => {
    setFilters((f) => {
      const cur = f.workflowIds ?? workflows.map((w) => w.id);
      const next = cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id];
      return { ...f, workflowIds: next.length === workflows.length ? null : next };
    });
  };

  const depCount = graph.links.filter((l) => l.kind === "dep").length;
  const hubCount = graph.nodes.filter((n) => n.kind === "hub").length;

  const chipBase =
    "inline-flex cursor-pointer items-center gap-1 rounded-full border px-2 py-0.5 text-xs transition-colors";
  const chipOn = "border-[var(--edge-strong)] bg-[var(--surface-raised)] text-[var(--t1)]";
  const chipOff = "border-[var(--edge)] text-[var(--t5)] opacity-60";

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="graph-page">
      {/* 顶栏：标题 + 统计 + 搜索 + 侧栏开关 */}
      <div className="flex items-center gap-3 border-b border-[var(--edge)] px-4 py-2.5">
        <Waypoints size={16} className="shrink-0 text-[var(--t3)]" aria-hidden />
        <h2 className="text-sm font-semibold text-[var(--t2)]">任务图谱</h2>
        <span className="text-xs text-[var(--t5)]" data-testid="graph-stats">
          {graph.nodes.filter((n) => n.kind === "task").length} 任务 · {depCount} 依赖 ·{" "}
          {hubCount} 工作流 · {ownerChips.length} 成员
        </span>
        <div className="flex-1" />
        <input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="搜索任务/标签…"
          aria-label="搜索图谱节点"
          className="nm-input h-7 w-56 rounded-[var(--r-sm)] px-2 text-xs"
        />
        <button
          className="nm-btn rounded-[var(--r-sm)] px-2 py-1 text-xs text-[var(--t3)]"
          onClick={() => setShowFilters((v) => !v)}
        >
          {showFilters ? "隐藏过滤器" : "显示过滤器"}
        </button>
      </div>

      <div className="flex min-h-0 flex-1">
        <div className="relative min-w-0 flex-1">
          <GraphCanvas
            graph={graph}
            colorMode={colorMode}
            selectedId={selectedId}
            hoverId={hoverId}
            searchMatchIds={searchMatchIds}
            onHover={setHoverId}
            onSelect={setSelectedId}
            onOpenHub={() => onOpenWorkflow()}
          />
          {/* 图例（着色模式切换内嵌） */}
          <div className="nm-card absolute left-3 top-3 flex items-center gap-2 rounded-[var(--r-md)] px-2.5 py-1.5 text-xs">
            {(["status", "owner"] as const).map((m) => (
              <button
                key={m}
                onClick={() => setColorMode(m)}
                className={
                  colorMode === m
                    ? "rounded-[var(--r-sm)] bg-[var(--inset-bg)] px-1.5 py-0.5 font-medium text-[var(--t1)]"
                    : "px-1.5 py-0.5 text-[var(--t5)] hover:text-[var(--t2)]"
                }
              >
                {m === "status" ? "按状态" : "按成员"}
              </button>
            ))}
            {colorMode === "status" && (
              <span className="flex items-center gap-2 pl-1 text-[var(--t5)]">
                <i className="inline-block size-2 rounded-full bg-[var(--t5)]" />待办
                <i className="inline-block size-2 rounded-full bg-[var(--brand)]" />进行中
                <i className="inline-block size-2 rounded-full bg-[var(--success)]" />已完成
              </span>
            )}
          </div>
          {graph.nodes.length === 0 && (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
              <div className="nm-card rounded-[var(--r-lg)] px-6 py-4 text-center">
                <p className="text-sm font-medium text-[var(--t2)]">
                  {graph.totalTasks === 0 ? "暂无任务" : "当前过滤器下没有节点"}
                </p>
                <p className="mt-1 text-xs text-[var(--t5)]">
                  {graph.totalTasks === 0
                    ? "创建任务卡或导入他人的任务数据后，这里会长出整个部门的任务关系图"
                    : "试着放宽状态/成员/标签过滤器，或打开「孤立节点」"}
                </p>
              </div>
            </div>
          )}
        </div>

        {/* 右侧：详情面板 + 过滤器侧栏 */}
        {showFilters && (
          <aside className="w-64 shrink-0 overflow-y-auto border-l border-[var(--edge)] p-3 text-sm">
            {selectedNode?.task && (
              <div className="nm-card mb-3 rounded-[var(--r-md)] p-3" data-testid="graph-detail">
                <p className="text-sm font-medium text-[var(--t1)]">{selectedNode.label}</p>
                <dl className="mt-2 space-y-1 text-xs text-[var(--t3)]">
                  <div className="flex justify-between gap-2">
                    <dt className="text-[var(--t5)]">状态</dt>
                    <dd>
                      {STATUS_LABELS[selectedNode.status ?? "todo"]}
                      {selectedNode.archived ? "（已归档）" : ""}
                    </dd>
                  </div>
                  <div className="flex justify-between gap-2">
                    <dt className="text-[var(--t5)]">归属</dt>
                    <dd>
                      {ownerChips.find((o) => o.id === (selectedNode.owner ?? SELF_OWNER))
                        ?.name ?? "未知成员"}
                    </dd>
                  </div>
                  {selectedNode.task.due && (
                    <div className="flex justify-between gap-2">
                      <dt className="text-[var(--t5)]">截止</dt>
                      <dd>{selectedNode.task.due}</dd>
                    </div>
                  )}
                  {selectedNode.completedAt && (
                    <div className="flex justify-between gap-2">
                      <dt className="text-[var(--t5)]">完成于</dt>
                      <dd>
                        {new Date(selectedNode.completedAt).toLocaleDateString()}
                      </dd>
                    </div>
                  )}
                  {(selectedNode.tags ?? []).length > 0 && (
                    <div className="flex flex-wrap justify-end gap-1 pt-0.5">
                      {selectedNode.tags!.map((t) => (
                        <span key={t} className="nm-tag rounded-full px-1.5 text-[10px]">
                          {t}
                        </span>
                      ))}
                    </div>
                  )}
                </dl>
                {selectedNode.task.note && (
                  <p className="mt-2 line-clamp-4 text-xs text-[var(--t4)]">
                    {selectedNode.task.note}
                  </p>
                )}
                {selectedNode.task.result?.summary && (
                  <p className="mt-2 line-clamp-3 rounded-[var(--r-sm)] bg-[var(--inset-bg)] p-1.5 text-xs text-[var(--t4)]">
                    {selectedNode.task.result.summary}
                  </p>
                )}
                <div className="mt-2.5 flex gap-1.5">
                  {!selectedNode.owner && (
                    <button
                      className="nm-btn flex-1 rounded-[var(--r-sm)] px-2 py-1 text-xs"
                      onClick={() => onOpenTask(selectedNode.task!)}
                    >
                      在看板打开
                    </button>
                  )}
                  {selectedNode.workflowId && (
                    <button
                      className="nm-btn flex-1 rounded-[var(--r-sm)] px-2 py-1 text-xs"
                      onClick={onOpenWorkflow}
                    >
                      打开工作流
                    </button>
                  )}
                </div>
              </div>
            )}

            <FilterGroup title="状态">
              <div className="flex gap-1.5">
                {(Object.keys(filters.status) as Array<keyof GraphFilters["status"]>).map(
                  (s) => (
                    <button
                      key={s}
                      onClick={() =>
                        setFilters((f) => ({
                          ...f,
                          status: { ...f.status, [s]: !f.status[s] },
                        }))
                      }
                      className={`${chipBase} ${filters.status[s] ? chipOn : chipOff}`}
                    >
                      {STATUS_LABELS[s]}
                    </button>
                  )
                )}
              </div>
            </FilterGroup>

            <FilterGroup title="成员">
              <div className="flex flex-wrap gap-1.5">
                {ownerChips.map((o) => (
                  <button
                    key={o.id || "self"}
                    onClick={() => toggleOwner(o.id)}
                    className={`${chipBase} ${
                      (filters.owners ?? ownerChips.map((x) => x.id)).includes(o.id)
                        ? chipOn
                        : chipOff
                    }`}
                  >
                    {colorMode === "owner" && !o.isSelf && (
                      <i
                        className="inline-block size-2 rounded-full"
                        style={{ background: ownerColors.get(o.id) }}
                      />
                    )}
                    {o.name}
                  </button>
                ))}
              </div>
            </FilterGroup>

            {years.length > 0 && (
              <FilterGroup title="完成年份">
                <div className="flex flex-wrap gap-1.5">
                  <button
                    onClick={() => setFilters((f) => ({ ...f, year: null }))}
                    className={`${chipBase} ${filters.year === null ? chipOn : chipOff}`}
                  >
                    全部
                  </button>
                  {years.map((y) => (
                    <button
                      key={y}
                      onClick={() =>
                        setFilters((f) => ({ ...f, year: f.year === y ? null : y }))
                      }
                      className={`${chipBase} ${filters.year === y ? chipOn : chipOff}`}
                    >
                      {y}
                    </button>
                  ))}
                </div>
              </FilterGroup>
            )}

            {tagList.length > 0 && (
              <FilterGroup title={`标签（${tagList.length}）`}>
                <div className="flex flex-wrap gap-1.5">
                  {tagList.slice(0, 30).map(({ tag, count }) => (
                    <button
                      key={tag}
                      onClick={() => toggleTag(tag)}
                      className={`${chipBase} ${
                        (filters.tags ?? []).includes(tag) ? chipOn : chipOff
                      }`}
                    >
                      {tag}
                      <span className="text-[10px] text-[var(--t5)]">{count}</span>
                    </button>
                  ))}
                </div>
              </FilterGroup>
            )}

            {workflows.length > 0 && (
              <FilterGroup title={`工作流（${workflows.length}）`}>
                <div className="flex flex-col gap-1">
                  {workflows.map((w) => (
                    <label
                      key={w.id}
                      className="flex cursor-pointer items-center gap-2 text-xs text-[var(--t3)]"
                    >
                      <input
                        type="checkbox"
                        checked={
                          (filters.workflowIds ?? workflows.map((x) => x.id)).includes(w.id)
                        }
                        onChange={() => toggleWorkflow(w.id)}
                      />
                      <span className="truncate">{w.name}</span>
                    </label>
                  ))}
                </div>
              </FilterGroup>
            )}

            <FilterGroup title="显示">
              <label className="flex cursor-pointer items-center justify-between text-xs text-[var(--t3)]">
                孤立节点（无边任务）
                <input
                  type="checkbox"
                  checked={filters.includeOrphans}
                  onChange={() =>
                    setFilters((f) => ({ ...f, includeOrphans: !f.includeOrphans }))
                  }
                />
              </label>
            </FilterGroup>

            <button
              className="mt-2 w-full rounded-[var(--r-sm)] px-2 py-1 text-xs text-[var(--t5)] hover:text-[var(--t2)]"
              onClick={() => setFilters(DEFAULT_FILTERS)}
            >
              重置过滤器
            </button>
          </aside>
        )}
      </div>
    </div>
  );
}

function FilterGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mb-3">
      <h3 className="mb-1.5 text-xs font-medium text-[var(--t5)]">{title}</h3>
      {children}
    </section>
  );
}
