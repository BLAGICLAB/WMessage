// 工作流画布图论工具（W1-CANVAS，设计 §5/§9）：
// 连线校验（环检测）与 dagre 自动布局。纯函数，无 React/Tauri 依赖。
import dagre from "@dagrejs/dagre";

/** 画布节点（草稿态）：localId 是画布内部拓扑键；
 *  taskId = 已保存真实任务 id（保存后由后端绑定回写，可能因指纹替换而变化） */
export interface CanvasNode {
  localId: string;
  taskId?: string;
  title: string;
  note?: string;
  tags?: string[];
  dependsOn: string[];
  pos: { x: number; y: number };
}

/** 画布上任务卡的固定逻辑尺寸（ dagre 布局 + 拖动命中用；实际渲染宽 ~340px） */
export const NODE_WIDTH = 340;
export const NODE_HEIGHT = 150;

/**
 * 加边 from→to（to 的 dependsOn 增加/from 为上游）是否会产生环。
 * 环存在 ⇔ 从 to 出发沿已有边可达 from。
 */
export function wouldCreateCycle(
  nodes: Pick<CanvasNode, "localId" | "dependsOn">[],
  from: string,
  to: string
): boolean {
  if (from === to) return true;
  // dependsOn[k] = k 的上游；因此 k 的下游是所有 dependsOn 包含 k 的节点
  const down = new Map<string, string[]>();
  for (const n of nodes)
    for (const d of n.dependsOn) {
      const list = down.get(d) ?? [];
      list.push(n.localId);
      down.set(d, list);
    }
  const seen = new Set<string>([to]);
  const stack = [to];
  while (stack.length) {
    const cur = stack.pop()!;
    if (cur === from) return true;
    for (const next of down.get(cur) ?? []) {
      if (!seen.has(next)) {
        seen.add(next);
        stack.push(next);
      }
    }
  }
  return false;
}

/** dagre 自上而下分层布局（设计 §9）：用于生成初版/加载无坐标的保存结果 */
export function layoutGraph(
  nodes: Pick<CanvasNode, "localId" | "dependsOn">[]
): Record<string, { x: number; y: number }> {
  const g = new dagre.graphlib.Graph();
  g.setGraph({ rankdir: "TB", nodesep: 60, ranksep: 90, marginx: 40, marginy: 40 });
  g.setDefaultEdgeLabel(() => ({}));
  const ids = new Set(nodes.map((n) => n.localId));
  for (const n of nodes) g.setNode(n.localId, { width: NODE_WIDTH, height: NODE_HEIGHT });
  for (const n of nodes)
    for (const d of n.dependsOn) {
      // 悬空引用防御：布局只画画布内存在的边
      if (ids.has(d)) g.setEdge(d, n.localId);
    }
  dagre.layout(g);
  const out: Record<string, { x: number; y: number }> = {};
  for (const n of nodes) {
    // dagre 返回的是节点中心；React Flow 用左上角
    const pos = g.node(n.localId);
    out[n.localId] = {
      x: pos.x - NODE_WIDTH / 2,
      y: pos.y - NODE_HEIGHT / 2,
    };
  }
  return out;
}

/** 从已保存的工作流任务行重建画布节点（localId = 真实任务 id；
 *  dependsOn 里的 id 天然是 localId；缺坐标的行补 dagre 布局） */
export function draftFromTasks(
  tasks: Array<
    Pick<
      import("../../types").Task,
      "id" | "title" | "note" | "tags" | "dependsOn" | "canvasPos" | "workflowId"
    >
  >
): CanvasNode[] {
  const base: CanvasNode[] = tasks.map((t) => ({
    localId: t.id,
    taskId: t.id,
    title: t.title,
    note: t.note,
    tags: t.tags,
    dependsOn: [...(t.dependsOn ?? [])],
    pos: t.canvasPos ?? { x: 0, y: 0 },
  }));
  // 任一行缺坐标 → 全图重排；全有坐标（用户手拖过）则原样保留
  const missingPos = tasks.some((t) => t.canvasPos == null);
  if (missingPos) {
    const laid = layoutGraph(base);
    for (const n of base) n.pos = laid[n.localId] ?? n.pos;
  }
  return base;
}
