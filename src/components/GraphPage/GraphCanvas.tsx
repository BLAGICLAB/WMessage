// 图谱画布（任务图谱设计 §3.3）：Canvas 2D 逐帧渲染 + 力导向物理 + 相机平移缩放
// + hover 邻接高亮（其余淡出）/ 点选 / 节点拖拽固定。配色读 CSS 变量（双主题自动
// 适配，MutationObserver 监听 html.dark 切换重读）；物理循环 alpha 冷却后自动停。

import { useEffect, useRef } from "react";
import type { BuiltGraph, GraphColorMode, GraphNode } from "./graph-build";
import {
  DEFAULT_CONFIG,
  LINK_DEP,
  LINK_MEMBER,
  REHEAT_ALPHA,
  initPositions,
  rebindPositions,
  taskRadius,
  tick,
  type PhysLink,
  type PhysNode,
} from "./physics";

interface Palette {
  bg: string;
  t1: string;
  t2: string;
  t3: string;
  t5: string;
  brand: string;
  brandStrong: string;
  success: string;
  edge: string;
  edgeStrong: string;
  danger: string;
}

function readPalette(): Palette {
  const s = getComputedStyle(document.documentElement);
  const v = (name: string, fallback: string) =>
    s.getPropertyValue(name).trim() || fallback;
  return {
    bg: v("--bg", "#eef1f6"),
    t1: v("--t1", "#182031"),
    t2: v("--t2", "#2b3548"),
    t3: v("--t3", "#45506a"),
    t5: v("--t5", "#5d6984"),
    brand: v("--brand", "#5f6f94"),
    brandStrong: v("--brand-strong", "#4a5879"),
    success: v("--success", "#16a34a"),
    edge: v("--edge", "#e2e7ef"),
    edgeStrong: v("--edge-strong", "#cbd4e1"),
    danger: v("--danger", "#dc2626"),
  };
}

/** 成员着色调色盘（owner 模式外来成员按序取色；与主题协调的低饱和系） */
export const OWNER_PALETTE = [
  "#7c6bd6",
  "#c2711d",
  "#0e7490",
  "#b04a8f",
  "#5a7a1f",
  "#b8434a",
  "#6b7dd6",
  "#8a6d3b",
];

export interface GraphCanvasProps {
  graph: BuiltGraph;
  colorMode: GraphColorMode;
  /** ownerKey → 颜色（owner 模式；SELF_OWNER 键 = 本人） */
  ownerColors: Map<string, string>;
  selectedId: string | null;
  hoverId: string | null;
  searchMatchIds: Set<string> | null;
  onHover: (id: string | null) => void;
  onSelect: (id: string | null) => void;
  /** 双击 hub → 打开工作流 */
  onOpenHub: (workflowId: string) => void;
}

interface Camera {
  x: number;
  y: number;
  scale: number;
}

export default function GraphCanvas(props: GraphCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const nodesRef = useRef<PhysNode[]>([]);
  const linksRef = useRef<PhysLink[]>([]);
  const indexRef = useRef<Map<string, number>>(new Map());
  const cameraRef = useRef<Camera>({ x: 0, y: 0, scale: 1 });
  const alphaRef = useRef(0);
  const rafRef = useRef(0);
  const paletteRef = useRef<Palette>(readPalette());
  const propsRef = useRef(props);
  propsRef.current = props;
  // 指针状态：空白拖拽 = 平移，节点拖拽 = 固定点
  const dragRef = useRef<
    | { type: "pan"; startX: number; startY: number; camX: number; camY: number }
    | { type: "node"; id: string; sx: number; sy: number }
    | { type: "none"; moved: boolean }
    | null
  >(null);
  const sizeRef = useRef({ w: 0, h: 0 });
  const paintKeyRef = useRef("");
  const graphVersionRef = useRef(0);
  const paintFrameRef = useRef<() => void>(() => {});

  // ── 建图数据 → 物理节点（按 id 保留位置；图变更暖启动） ──
  useEffect(() => {
    const { graph } = props;
    const specs = graph.nodes.map((n) => ({
      id: n.id,
      r: n.kind === "hub" ? 7 + Math.sqrt(n.degree) : taskRadius(n.degree),
    }));
    const { w, h } = sizeRef.current;
    nodesRef.current =
      nodesRef.current.length === 0
        ? initPositions(specs, w || 800, h || 600)
        : rebindPositions(nodesRef.current, specs, w || 800, h || 600);
    indexRef.current = new Map(nodesRef.current.map((n, i) => [n.id, i]));
    linksRef.current = [];
    for (const l of graph.links) {
      const s = indexRef.current.get(l.source);
      const t = indexRef.current.get(l.target);
      if (s === undefined || t === undefined) continue;
      const p = l.kind === "dep" ? LINK_DEP : LINK_MEMBER;
      linksRef.current.push({
        source: s,
        target: t,
        distance: p.distance,
        strength: p.strength,
      });
    }
    // 预稳定：建图/改过滤后先同步跑固定 tick，首帧即接近收敛——
    // 否则弱引力还没把整团拉到中心 alpha 就冷却了，布局会冻在半路
    const w0 = (w || 800) / 2;
    const h0 = (h || 600) / 2;
    let a = 1;
    for (let i = 0; i < 150; i++) {
      a = tick(nodesRef.current, linksRef.current, DEFAULT_CONFIG, a, w0, h0);
    }
    alphaRef.current = Math.max(a, REHEAT_ALPHA);
    // 内容签名：节点数相同但标签/数据变了（如异步工作流名到达）也要重绘
    graphVersionRef.current += 1;
  }, [props.graph]);

  // ── 主题切换重读配色（切主题不经 React 重渲染，观察后须立即重绘） ──
  useEffect(() => {
    const obs = new MutationObserver(() => {
      paletteRef.current = readPalette();
      paintFrameRef.current();
    });
    obs.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    return () => obs.disconnect();
  }, []);

  // ── 物理循环 + 渲染（合并尺寸同步：隐藏窗口/节流环境下 rAF 与 RO 均不投递，
  //    建图后同步 syncSize + paintFrame 保证图不依赖帧调度也能出来） ──
  useEffect(() => {
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!canvas || !wrap) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return; // jsdom/测试环境无 2D 上下文：静默跳过（逻辑由纯函数单测覆盖）

    // 位图尺寸与 CSS 尺寸同步（含首次尺寸的铺点归中平移）；无变化返回 false
    const syncSize = () => {
      const rect = wrap.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      const prev = sizeRef.current;
      if (Math.abs(rect.width - prev.w) < 0.5 && Math.abs(rect.height - prev.h) < 0.5) {
        return false;
      }
      sizeRef.current = { w: rect.width, h: rect.height };
      canvas.width = Math.max(1, Math.round(rect.width * dpr));
      canvas.height = Math.max(1, Math.round(rect.height * dpr));
      canvas.style.width = `${rect.width}px`;
      canvas.style.height = `${rect.height}px`;
      // 首次拿到真实尺寸：初始铺点用的是 800×600 假设，整体平移到真实中心
      if (prev.w === 0 && prev.h === 0 && rect.width > 0) {
        const dx = rect.width / 2 - 400;
        const dy = rect.height / 2 - 300;
        if (dx !== 0 || dy !== 0) {
          for (const n of nodesRef.current) {
            if (n.fx === null) n.x += dx;
            if (n.fy === null) n.y += dy;
          }
        }
      }
      return true;
    };

    const ro =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => {
            if (syncSize()) {
              alphaRef.current = Math.max(alphaRef.current, REHEAT_ALPHA);
              paintFrame();
            }
          });
    ro?.observe(wrap);

    const nodeColor = (n: GraphNode): string => {
      const p = paletteRef.current;
      if (propsRef.current.colorMode === "owner") {
        return propsRef.current.ownerColors.get(n.owner ?? "") ?? p.t5;
      }
      if (n.kind === "hub") return p.t3;
      if (n.status === "done") return p.success;
      if (n.status === "doing") return p.brand;
      return p.t5;
    };

    const paintFrame = () => {
      const { graph, selectedId, hoverId, searchMatchIds } = propsRef.current;
      const p = paletteRef.current;
      const dpr = window.devicePixelRatio || 1;
      const { w, h } = sizeRef.current;
      const cam = cameraRef.current;
      const nodes = nodesRef.current;
      const index = indexRef.current;

      // 物理：alpha 冷却后停步（渲染照常，交互暖启动）；size 未就绪时不算（向心无中心）
      let alpha = alphaRef.current;
      if (alpha > DEFAULT_CONFIG.alphaMin && w > 0 && h > 0) {
        alpha = tick(
          nodes,
          linksRef.current,
          DEFAULT_CONFIG,
          alpha,
          w / 2,
          h / 2
        );
        alphaRef.current = alpha;
      }

      // hover/选中 → 邻接集合（其余淡出）
      const focusId = hoverId ?? selectedId;
      let neighbors: Set<string> | null = null;
      if (focusId) {
        neighbors = new Set([focusId]);
        for (const l of graph.links) {
          if (l.source === focusId) neighbors.add(l.target);
          if (l.target === focusId) neighbors.add(l.source);
        }
      }

      // 脏检查：物理冷却且相机/交互/数据签名未变时跳过绘制（省 CPU，也避免
      // 连续重绘让宿主截屏/合成器拿不到稳定帧）
      const paintKey = `v${graphVersionRef.current}|${alphaRef.current.toFixed(3)}|${cam.x.toFixed(1)}|${cam.y.toFixed(1)}|${cam.scale.toFixed(3)}|${focusId ?? ""}|${searchMatchIds?.size ?? 0}|${propsRef.current.colorMode}|${graph.nodes.length}|${w}x${h}|${p.bg}`;
      if (paintKey === paintKeyRef.current) {
        rafRef.current = requestAnimationFrame(draw);
        return;
      }
      paintKeyRef.current = paintKey;

      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.fillStyle = p.bg;
      ctx.fillRect(0, 0, w, h);
      ctx.save();
      ctx.translate(cam.x, cam.y);
      ctx.scale(cam.scale, cam.scale);

      const toScreen = (n: PhysNode) => ({
        x: n.x,
        y: n.y,
      });

      // 连线（dep 带箭头；member 细淡）
      for (const l of graph.links) {
        const s = index.get(l.source);
        const t = index.get(l.target);
        if (s === undefined || t === undefined) continue;
        const a = nodes[s];
        const b = nodes[t];
        const dimmed =
          neighbors !== null &&
          !neighbors.has(l.source) &&
          !neighbors.has(l.target);
        ctx.globalAlpha = dimmed ? 0.06 : l.kind === "member" ? 0.35 : 0.55;
        ctx.strokeStyle = l.kind === "dep" ? p.edgeStrong : p.edge;
        ctx.lineWidth = (l.kind === "dep" ? 1.2 : 0.8) / cam.scale;
        ctx.beginPath();
        const pa = toScreen(a);
        const pb = toScreen(b);
        // 终点收进节点半径，给箭头留位
        const dx = pb.x - pa.x;
        const dy = pb.y - pa.y;
        const d = Math.sqrt(dx * dx + dy * dy) || 1;
        const trim = b.r + (l.kind === "dep" ? 3 : 2);
        const ex = pb.x - (dx / d) * trim;
        const ey = pb.y - (dy / d) * trim;
        ctx.moveTo(pa.x + (dx / d) * a.r, pa.y + (dy / d) * a.r);
        ctx.lineTo(ex, ey);
        ctx.stroke();
        if (l.kind === "dep") {
          // 箭头（指向下游）
          const ah = 4 / cam.scale + 2;
          const ux = dx / d;
          const uy = dy / d;
          ctx.fillStyle = p.edgeStrong;
          ctx.beginPath();
          ctx.moveTo(ex, ey);
          ctx.lineTo(ex - ux * ah - uy * ah * 0.5, ey - uy * ah + ux * ah * 0.5);
          ctx.lineTo(ex - ux * ah + uy * ah * 0.5, ey - uy * ah - ux * ah * 0.5);
          ctx.closePath();
          ctx.fill();
        }
      }

      // 节点 + 标签
      const showAllLabels = cam.scale >= 0.9;
      for (const n of graph.nodes) {
        const i = index.get(n.id);
        if (i === undefined) continue;
        const pn = nodes[i];
        const isFocus = focusId === n.id;
        const isNeighbor = neighbors?.has(n.id) ?? false;
        const dimmed = neighbors !== null && !isNeighbor;
        const doneFade = n.kind === "task" && n.status === "done" && propsRef.current.colorMode === "status";
        const baseAlpha = dimmed ? 0.12 : doneFade ? 0.55 : 1;
        const sp = toScreen(pn);
        const ringSearch = searchMatchIds?.has(n.id) ?? false;

        if (isFocus) {
          ctx.globalAlpha = 1;
          ctx.strokeStyle = p.brandStrong;
          ctx.lineWidth = 2 / cam.scale;
          ctx.beginPath();
          ctx.arc(sp.x, sp.y, pn.r + 4 / cam.scale, 0, Math.PI * 2);
          ctx.stroke();
        } else if (ringSearch) {
          ctx.globalAlpha = 1;
          ctx.strokeStyle = p.danger;
          ctx.lineWidth = 1.6 / cam.scale;
          ctx.beginPath();
          ctx.arc(sp.x, sp.y, pn.r + 4 / cam.scale, 0, Math.PI * 2);
          ctx.stroke();
        }

        ctx.globalAlpha = baseAlpha;
        ctx.fillStyle = nodeColor(n);
        ctx.beginPath();
        ctx.arc(sp.x, sp.y, pn.r, 0, Math.PI * 2);
        ctx.fill();
        if (n.kind === "hub") {
          ctx.strokeStyle = p.edgeStrong;
          ctx.lineWidth = 1.5 / cam.scale;
          ctx.stroke();
        }

        // 标签：缩放足够 / 大节点 / 焦点邻域时显示
        const labelVisible =
          showAllLabels ||
          pn.r * cam.scale > 9 ||
          (isNeighbor && neighbors !== null);
        if (labelVisible) {
          ctx.globalAlpha = dimmed ? 0.1 : doneFade ? 0.6 : 0.95;
          ctx.fillStyle = n.kind === "hub" ? p.t1 : p.t2;
          const fontSize = Math.max(10, 11 / Math.sqrt(cam.scale));
          ctx.font = `${n.kind === "hub" ? "600 " : ""}${fontSize}px ui-sans-serif, system-ui, -apple-system, "PingFang SC", sans-serif`;
          ctx.textAlign = "center";
          ctx.textBaseline = "top";
          const label =
            n.label.length > 24 ? `${n.label.slice(0, 24)}…` : n.label;
          ctx.fillText(label, sp.x, sp.y + pn.r + 3);
        }
      }
      ctx.restore();
      ctx.globalAlpha = 1;
    };
    const draw = () => {
      paintFrame();
      rafRef.current = requestAnimationFrame(draw);
    };
    rafRef.current = requestAnimationFrame(draw);
    // 立即首绘：隐藏窗口/节流环境下 rAF 不跑（RO 也不投递），图必须照样出来。
    paintFrameRef.current = () => {
      syncSize();
      paintFrame();
    };
    paintFrameRef.current();
    return () => cancelAnimationFrame(rafRef.current);
  }, []);

  // 状态驱动重绘：每次渲染提交（hover/选中/过滤器/主题等）后立即同步重绘——
  // 不依赖 rAF（隐藏窗口/节流环境下也能即时刻画交互反馈）
  useEffect(() => {
    paintFrameRef.current();
  });

  // ── 指针交互 ──
  const pickNode = (mx: number, my: number): string | null => {
    const cam = cameraRef.current;
    const wx = (mx - cam.x) / cam.scale;
    const wy = (my - cam.y) / cam.scale;
    let best: { id: string; d: number } | null = null;
    for (const n of nodesRef.current) {
      const dx = n.x - wx;
      const dy = n.y - wy;
      const d = Math.sqrt(dx * dx + dy * dy);
      const hit = n.r + 4 / cam.scale;
      if (d <= hit && (!best || d < best.d)) best = { id: n.id, d };
    }
    return best?.id ?? null;
  };

  const localPoint = (e: React.PointerEvent | React.MouseEvent) => {
    const rect = canvasRef.current!.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  };

  return (
    <div ref={wrapRef} className="relative h-full w-full overflow-hidden">
      <canvas
        ref={canvasRef}
        className="block h-full w-full cursor-grab active:cursor-grabbing touch-none"
        data-testid="graph-canvas"
        onPointerDown={(e) => {
          const { x, y } = localPoint(e);
          const hit = pickNode(x, y);
          // 合成事件（无活动指针）setPointerCapture 会抛 NotFoundError，捕获即可
          try {
            e.currentTarget.setPointerCapture(e.pointerId);
          } catch {
            /* ignore */
          }
          if (hit) {
            const i = indexRef.current.get(hit);
            const n = i === undefined ? null : nodesRef.current[i];
            if (n) {
              n.fx = n.x;
              n.fy = n.y;
            }
            dragRef.current = { type: "node", id: hit, sx: x, sy: y };
          } else {
            const cam = cameraRef.current;
            dragRef.current = {
              type: "pan",
              startX: x,
              startY: y,
              camX: cam.x,
              camY: cam.y,
            };
          }
        }}
        onPointerMove={(e) => {
          const { x, y } = localPoint(e);
          const drag = dragRef.current;
          if (!drag) {
            props.onHover(pickNode(x, y));
            return;
          }
          if (drag.type === "pan") {
            const cam = cameraRef.current;
            cam.x = drag.camX + (x - drag.startX);
            cam.y = drag.camY + (y - drag.startY);
          } else if (drag.type === "node") {
            const cam = cameraRef.current;
            const i = indexRef.current.get(drag.id);
            const n = i === undefined ? null : nodesRef.current[i];
            if (n) {
              n.fx = (x - cam.x) / cam.scale;
              n.fy = (y - cam.y) / cam.scale;
              alphaRef.current = Math.max(alphaRef.current, REHEAT_ALPHA);
            }
            props.onHover(drag.id);
          }
        }}
        onPointerUp={(e) => {
          const drag = dragRef.current;
          dragRef.current = null;
          const { x, y } = localPoint(e);
          if (drag?.type === "node") {
            const moved =
              Math.abs(x - drag.sx) > 3 || Math.abs(y - drag.sy) > 3;
            if (!moved) props.onSelect(drag.id);
          } else if (drag?.type === "pan") {
            const moved =
              Math.abs(x - drag.startX) > 3 || Math.abs(y - drag.startY) > 3;
            if (!moved) props.onSelect(pickNode(x, y));
          }
        }}
        onPointerLeave={() => {
          props.onHover(null);
        }}
        onDoubleClick={(e) => {
          const { x, y } = localPoint(e);
          const hit = pickNode(x, y);
          if (!hit) return;
          const node = props.graph.nodes.find((n) => n.id === hit);
          if (node?.kind === "hub" && node.workflowId) props.onOpenHub(node.workflowId);
        }}
        onWheel={(e) => {
          e.preventDefault();
          const cam = cameraRef.current;
          const rect = canvasRef.current!.getBoundingClientRect();
          const mx = e.clientX - rect.left;
          const my = e.clientY - rect.top;
          const factor = Math.exp(-e.deltaY * 0.0015);
          const next = Math.min(4, Math.max(0.15, cam.scale * factor));
          // 指针为锚缩放
          cam.x = mx - ((mx - cam.x) * next) / cam.scale;
          cam.y = my - ((my - cam.y) * next) / cam.scale;
          cam.scale = next;
        }}
      />
    </div>
  );
}
