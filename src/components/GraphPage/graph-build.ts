// 任务图谱构建（任务图谱设计 §3.1）：tasks + workflows → 节点/边/过滤器。
// 纯函数，无 React/Tauri 依赖（对齐 WorkflowCanvas/graph.ts 惯例，单测锚点）。
// 统计口径：一切按 completedAt（完成时间）归年——归档只是完成 7 天后的存储状态，
// 按归档时间会把跨年完成的任务记错年份（设计 §3.1）。

import type { ColumnId, Task, Workflow } from "../../types";

export type GraphColorMode = "status" | "owner";

/** 本人 ownerKey 哨兵（Task.ownerId 为 undefined = 本人） */
export const SELF_OWNER = "";

export interface GraphFilters {
  status: Record<ColumnId, boolean>;
  /** 命中的 ownerKey 集合；null = 全部 */
  owners: string[] | null;
  /** 命中任一标签即保留；null = 不限 */
  tags: string[] | null;
  /** 工作流白名单（只约束工作流卡）；null = 不限 */
  workflowIds: string[] | null;
  /** completedAt 年份；null = 不限 */
  year: number | null;
  /** 无边任务节点是否显示（Obsidian 的 orphans） */
  includeOrphans: boolean;
}

export const DEFAULT_FILTERS: GraphFilters = {
  status: { todo: true, doing: true, done: true },
  owners: null,
  tags: null,
  workflowIds: null,
  year: null,
  includeOrphans: true,
};

type GraphNodeKind = "task" | "hub";

export interface GraphNode {
  id: string;
  kind: GraphNodeKind;
  label: string;
  status?: ColumnId;
  archived?: boolean;
  /** ownerKey：undefined = 本人 */
  owner?: string;
  tags?: string[];
  workflowId?: string;
  completedAt?: number;
  /** 任务节点的完整卡引用（详情面板/跳转用） */
  task?: Task;
  /** 连接度数（dep 边 + member 边），驱动半径 */
  degree: number;
}

interface GraphLink {
  source: string;
  target: string;
  kind: "dep" | "member";
}

export interface BuiltGraph {
  nodes: GraphNode[];
  links: GraphLink[];
  /** 过滤前的全量任务数（统计条用） */
  totalTasks: number;
}

/** 成员 chips 数据（本人恒在最前，name 由调用方用 profile 兜底） */
export interface OwnerChip {
  id: string;
  name: string;
  isSelf: boolean;
}

export function collectOwners(tasks: Task[], people: OwnerChip[]): OwnerChip[] {
  const present = new Set<string>();
  for (const t of tasks) {
    if (!t.deletedAt) present.add(t.ownerId ?? SELF_OWNER);
  }
  const known = new Map(people.map((p) => [p.id, p]));
  const self = known.get(SELF_OWNER) ?? { id: SELF_OWNER, name: "我", isSelf: true };
  const out: OwnerChip[] = present.has(SELF_OWNER)
    ? [{ ...self, isSelf: true }]
    : [];
  for (const id of present) {
    if (id === SELF_OWNER) continue;
    const p = known.get(id);
    out.push(p ? { ...p, isSelf: false } : { id, name: "未知成员", isSelf: false });
  }
  return out;
}

/** 标签按使用计数降序（多人汇总后自动去重聚合） */
export function collectTags(tasks: Task[]): Array<{ tag: string; count: number }> {
  const counts = new Map<string, number>();
  for (const t of tasks) {
    if (t.deletedAt) continue;
    for (const tag of t.tags ?? []) {
      const key = tag.trim();
      if (!key) continue;
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
  }
  return [...counts.entries()]
    .map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag));
}

/** 有完成时间的年份集合（降序） */
export function collectYears(tasks: Task[]): number[] {
  const years = new Set<number>();
  for (const t of tasks) {
    if (t.deletedAt || !t.completedAt) continue;
    years.add(new Date(t.completedAt).getFullYear());
  }
  return [...years].sort((a, b) => b - a);
}

function passTaskFilters(t: Task, f: GraphFilters): boolean {
  if (t.deletedAt) return false;
  if (!f.status[t.column]) return false;
  if (f.owners && !f.owners.includes(t.ownerId ?? SELF_OWNER)) return false;
  if (f.tags) {
    const tags = t.tags ?? [];
    if (!f.tags.some((x) => tags.includes(x))) return false;
  }
  if (f.year !== null) {
    if (!t.completedAt || new Date(t.completedAt).getFullYear() !== f.year) return false;
  }
  if (f.workflowIds && t.workflowId && !f.workflowIds.includes(t.workflowId)) {
    return false;
  }
  return true;
}

/**
 * 依赖环检测（G5-DEPEDIT 纯函数锚点）：若把 `depId` 加入 `selfId` 的 dependsOn
 * 是否成环。语义：A.dependsOn 含 B = B 是 A 的上游；因此从 depId 沿 dependsOn
 * 正向可达 selfId ⇒ 成环。含自环（depId === selfId）必拒；悬空 id 无环不拒。
 */
export function wouldCreateDepCycle(
  tasks: Array<Pick<Task, "id" | "dependsOn" | "deletedAt">>,
  selfId: string,
  depId: string
): boolean {
  if (selfId === depId) return true;
  const depsOf = new Map<string, string[]>();
  for (const t of tasks) {
    if (t.deletedAt) continue;
    const deps = (t.dependsOn ?? []).filter(Boolean);
    if (deps.length) depsOf.set(t.id, deps);
  }
  const seen = new Set<string>([depId]);
  const stack = [depId];
  while (stack.length) {
    const cur = stack.pop()!;
    if (cur === selfId) return true;
    for (const next of depsOf.get(cur) ?? []) {
      if (!seen.has(next)) {
        seen.add(next);
        stack.push(next);
      }
    }
  }
  return false;
}

/**
 * 建图（设计 §3.1）：节点 = 任务 + 工作流 hub；边 = dependsOn（有向 dep）+
 * 成员关系（member）。悬空 dependsOn / 被过滤端点 → 丢边；
 * hub 只为「有存活成员」的工作流创建。度数在成边后统计，
 * includeOrphans=false 时剪掉无边任务节点。
 */
export function buildTaskGraph(
  tasks: Task[],
  workflows: Workflow[],
  filters: GraphFilters
): BuiltGraph {
  const kept = tasks.filter((t) => passTaskFilters(t, filters));
  const nodes = new Map<string, GraphNode>();
  const links: GraphLink[] = [];
  const idSet = new Set(kept.map((t) => t.id));

  for (const t of kept) {
    nodes.set(t.id, {
      id: t.id,
      kind: "task",
      label: t.title,
      status: t.column,
      archived: t.archived,
      owner: t.ownerId ?? undefined,
      tags: t.tags,
      workflowId: t.workflowId,
      completedAt: t.completedAt,
      task: t,
      degree: 0,
    });
  }

  // hub：工作流卡 → wf:{id}（白名单生效时只建白名单内工作流的 hub）
  const allowedWorkflows = filters.workflowIds;
  const wfById = new Map(workflows.map((w) => [w.id, w]));
  for (const t of kept) {
    if (!t.workflowId) continue;
    if (allowedWorkflows && !allowedWorkflows.includes(t.workflowId)) continue;
    const hubId = `wf:${t.workflowId}`;
    if (!nodes.has(hubId)) {
      nodes.set(hubId, {
        id: hubId,
        kind: "hub",
        label: wfById.get(t.workflowId)?.name ?? "未命名工作流",
        workflowId: t.workflowId,
        degree: 0,
      });
    }
    links.push({ source: t.id, target: hubId, kind: "member" });
  }

  // 依赖边：两端都在图内才连（悬空引用/被过滤端点丢弃）
  for (const t of kept) {
    for (const d of t.dependsOn ?? []) {
      if (idSet.has(d)) links.push({ source: d, target: t.id, kind: "dep" });
    }
  }

  // 度数统计 + 孤立节点剪枝
  for (const l of links) {
    const s = nodes.get(l.source);
    const t = nodes.get(l.target);
    if (s) s.degree += 1;
    if (t) t.degree += 1;
  }
  if (!filters.includeOrphans) {
    for (const [id, n] of nodes) {
      if (n.kind === "task" && n.degree === 0) nodes.delete(id);
    }
    // 剪掉因剪枝而失去全部成员的 hub
    for (const [id, n] of nodes) {
      if (n.kind === "hub" && n.degree === 0) nodes.delete(id);
    }
  }

  return {
    nodes: [...nodes.values()],
    links: links.filter(
      (l) => nodes.has(l.source) && nodes.has(l.target)
    ),
    totalTasks: tasks.filter((t) => !t.deletedAt).length,
  };
}
