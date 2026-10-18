// 图谱速览卡：映射任务图谱页——同一 buildTaskGraph 数据源（任务节点 + 依赖边，
// 无工作流入参故无 hub），FA2 同步迭代布局后归一化进 SVG。
// 联动悬停：activeTaskId 高亮该任务节点、activeDay 高亮当天有排期/完成的节点，
// 其余节点与边降透明（Dim）；点节点跳任务页编辑态，右上角入口跳图谱页。

import { useMemo } from "react";
import Graph from "graphology";
import forceAtlas2, { inferSettings } from "graphology-layout-forceatlas2";
import { Network } from "lucide-react";
import type { Task } from "../../types";
import {
  DEFAULT_FILTERS,
  buildTaskGraph,
  type GraphNode,
} from "../GraphPage/graph-build";
import { dayKey as localDayKey } from "./homeData";

const VIEW_W = 100;
const VIEW_H = 56;

/**
 * FA2 设置（万节点档调优）。速览卡是 100×56 的二维缩略图，档位比图谱页粗一档：
 * barnesHut 只在真的上千节点时开，linLog 永久禁——LinLog 会把连通团坍缩成一点、
 * 孤点推到无穷远，与「所有任务可见铺开」不可调和。
 */
function fa2Settings(nodeCount: number) {
  return {
    ...inferSettings(Math.min(nodeCount, 5000)),
    barnesHutOptimize: nodeCount > 1500,
    edgeWeightInfluence: 1,
    linLogMode: false,
    outboundAttractionDistribution: false,
    adjustSizes: true,
    slowDown: 1 + Math.min(10, nodeCount / 500),
  };
}

/** 任务图 → FA2 布局 → 归一化坐标（memorize 于调用方） */
function layoutGraph(tasks: Task[]): {
  laid: Array<{ node: GraphNode; x: number; y: number }>;
  edges: Array<{ source: string; target: string }>;
  byId: Map<string, { x: number; y: number }>;
} {
  const built = buildTaskGraph(tasks, DEFAULT_FILTERS);
  const g = new Graph({ type: "undirected" });
  for (const n of built.nodes) {
    // 圆盘初值帮助 FA2 收敛（与图谱页分扇区初值同思路的简化版）
    const angle = (g.order / Math.max(built.nodes.length, 1)) * Math.PI * 2;
    g.addNode(n.id, { x: Math.cos(angle) * 10, y: Math.sin(angle) * 10, size: 1 });
  }
  for (const l of built.links) {
    if (g.hasNode(l.source) && g.hasNode(l.target) && !g.hasEdge(l.source, l.target))
      g.addEdge(l.source, l.target);
  }
  if (g.order > 0) {
    forceAtlas2.assign(g, {
      iterations: 80,
      settings: fa2Settings(g.order),
    });
  }
  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
  g.forEachNode((_, attrs) => {
    const x = Number(attrs.x) || 0;
    const y = Number(attrs.y) || 0;
    minX = Math.min(minX, x); maxX = Math.max(maxX, x);
    minY = Math.min(minY, y); maxY = Math.max(maxY, y);
  });
  const spanX = maxX - minX || 1;
  const spanY = maxY - minY || 1;
  const byId = new Map<string, { x: number; y: number }>();
  const laid = built.nodes.map((node) => {
    const attrs = g.getNodeAttributes(node.id);
    const x = 6 + ((attrs.x - minX) / spanX) * (VIEW_W - 12);
    const y = 5 + ((attrs.y - minY) / spanY) * (VIEW_H - 10);
    byId.set(node.id, { x, y });
    return { node, x, y };
  });
  const edges = built.links.map((l) => ({ source: l.source, target: l.target }));
  return { laid, edges, byId };
}

/** 节点是否落在联动指向的一天（当日有排期或当日完成） */
function nodeMatchesDay(node: GraphNode, dayKey: string): boolean {
  const t = node.task;
  if (!t) return false;
  if (t.planStart && t.planStart.slice(0, 10) === dayKey) return true;
  return (
    !!t.completedAt &&
    // 完成日按本地日键比较：new Date(dayKey) 按 UTC 解析，配 toDateString
    // 在西半球时区会整体偏一天，与 homeData 的本地 dayKey 口径冲突
    localDayKey(new Date(t.completedAt)) === dayKey
  );
}

export function GraphMini({
  tasks,
  dayKeys,
  activeDay,
  activeTaskId,
  onHoverTask,
  onOpenTask,
  onMore,
}: {
  tasks: Task[];
  /** 本周 7 天的日期键（与周图/日程/任务卡共用一套联动坐标） */
  dayKeys: string[];
  activeDay: number | null;
  activeTaskId: string | null;
  onHoverTask: (task: Task | null) => void;
  onOpenTask: (t: Task) => void;
  onMore: () => void;
}) {
  const { laid, edges, byId } = useMemo(() => layoutGraph(tasks), [tasks]);
  const dayKey = activeDay !== null ? dayKeys[activeDay] : null;

  const dimmed = (node: GraphNode): boolean => {
    if (activeTaskId) return node.id !== activeTaskId;
    if (dayKey) return !nodeMatchesDay(node, dayKey);
    return false;
  };

  return (
    <section className="nm-card flex h-full flex-col p-4" aria-label="图谱速览">
      <div className="flex items-center gap-2">
        <span
          aria-hidden
          className="h-[7px] w-[7px] shrink-0 rounded-full"
          style={{ background: "var(--plan-violet)" }}
        />
        <h3 className="text-[13px] font-semibold text-[var(--t3)]">图谱速览</h3>
        <button
          onClick={onMore}
          className="ml-auto rounded-md px-2 py-0.5 text-xs text-[var(--t4)] transition-colors hover:bg-[var(--hover-bg)] hover:text-[var(--t2)]"
        >
          图谱页
        </button>
      </div>

      {laid.length === 0 ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-2 py-6 text-center">
          <Network size={20} className="text-[var(--t5)]" aria-hidden />
          <p className="text-[13px] text-[var(--t5)]">
            还没有任务节点——建几张卡再回来看看关系网。
          </p>
        </div>
      ) : (
        <div className="relative mt-2 flex-1">
          <svg
            viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
            className="h-full w-full"
            preserveAspectRatio="xMidYMid meet"
            data-testid="graph-mini"
          >
            {edges.map((e) => {
              const s = byId.get(e.source);
              const t = byId.get(e.target);
              if (!s || !t) return null;
              const hot =
                activeTaskId === e.source || activeTaskId === e.target;
              return (
                <line
                  key={`${e.source}-${e.target}`}
                  x1={s.x}
                  y1={s.y}
                  x2={t.x}
                  y2={t.y}
                  stroke="var(--edge-strong)"
                  strokeWidth={hot ? 0.8 : 0.5}
                  opacity={activeTaskId ? (hot ? 0.9 : 0.12) : 0.6}
                />
              );
            })}
            {laid.map(({ node, x, y }) => {
              const dim = dimmed(node);
              const hot = activeTaskId === node.id;
              const lit = !dim && (activeTaskId !== null || activeDay !== null);
              const r = 2.2 + Math.min(node.degree, 8) * 0.55;
              // 速览卡留在应用主题里（它嵌在首页卡片中）；图谱页画布则是恒定深色
              // 星云，那边的状态色另走一套（graph-palette 的 STATUS_PALETTE）。
              // 两边不共用色值不是遗漏，是两张不同的屏。
              const fill =
                node.status === "done"
                  ? "var(--success)"
                  : "var(--plan-slate)";
              return (
                <circle
                  key={node.id}
                  cx={x}
                  cy={y}
                  r={hot ? r + 0.8 : r}
                  fill={lit ? "var(--accent)" : fill}
                  opacity={dim ? 0.22 : 1}
                  stroke={hot ? "var(--accent)" : "var(--surface)"}
                  strokeWidth={hot ? 0.9 : 0.4}
                  className="cursor-pointer"
                  data-testid={`graph-node-${node.id}`}
                  onMouseEnter={() => node.task && onHoverTask(node.task)}
                  onMouseLeave={() => onHoverTask(null)}
                  onClick={() => node.task && onOpenTask(node.task)}
                >
                  <title>{node.label}</title>
                </circle>
              );
            })}
          </svg>
        </div>
      )}
    </section>
  );
}
