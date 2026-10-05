// 图谱画布（G3-SIGMA 迁移）：Sigma.js v3 WebGL 渲染 + ForceAtlas2 worker 物理。
// 旧 Canvas2D 自研渲染/物理已删除（docs/batches/G3-SIGMA.spec.md）。
// 交互契约与旧版一致：hover 邻接高亮其余淡出、点选详情、空白拖拽平移、
// 节点拖拽固定、滚轮指针锚缩放、双击 hub 打开工作流、搜索描环、双主题。
// 配色读 CSS 变量（reducer 运行时解析，主题切换即重算），不硬编码色值。

import { useEffect, useRef } from "react";
import Graph from "graphology";
import Sigma from "sigma";
import type { Attributes } from "graphology-types";
import FA2Layout from "graphology-layout-forceatlas2/worker";
import { fa2Settings, toGraphologyGraph, type SigmaNodeAttrs } from "./graph-adapter";
import type { BuiltGraph, GraphColorMode } from "./graph-build";

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

/** 语义色键 → 色值（owner 模式外来成员的固定低饱和调色盘，与旧版一致） */
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

function resolveColor(
  key: string,
  palette: Palette,
  colorMode: GraphColorMode
): string {
  if (colorMode === "owner") {
    if (key === "self") return palette.brand;
    if (key.startsWith("owner:")) {
      const idx = Number(key.slice(6)) % OWNER_PALETTE.length;
      return OWNER_PALETTE[idx];
    }
  }
  switch (key) {
    case "hub":
      return palette.t3;
    case "doing":
      return palette.brand;
    case "done":
      return palette.success;
    default:
      return palette.t5; // todo
  }
}

export interface GraphCanvasProps {
  graph: BuiltGraph;
  colorMode: GraphColorMode;
  selectedId: string | null;
  hoverId: string | null;
  searchMatchIds: Set<string> | null;
  onHover: (id: string | null) => void;
  onSelect: (id: string | null) => void;
  /** 双击 hub → 打开工作流 */
  onOpenHub: (workflowId: string) => void;
}

export default function GraphCanvas(props: GraphCanvasProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const sigmaRef = useRef<Sigma<SigmaNodeAttrs> | null>(null);
  const fa2Ref = useRef<FA2Layout | null>(null);
  const propsRef = useRef(props);
  propsRef.current = props;
  const paletteRef = useRef<Palette>(readPalette());

  // ── 建图 → Sigma 实例 + FA2 worker（graph 变更时重建；旧实例 kill 释放内存） ──
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const g: Graph<SigmaNodeAttrs> = toGraphologyGraph(
      props.graph,
      props.colorMode,
      // owner 序从当前图内实际出现的 owner 推导（与 useMemo ownerOrder 同规则）
      [...new Set(props.graph.nodes.map((n) => n.owner ?? ""))].map((id) => ({
        id,
        name: id,
        isSelf: id === "",
      }))
    );

    // 初始相机适配在 Sigma 首帧后自动进行；先关标签渲染由 reducer 控制
    const sigma = new Sigma<SigmaNodeAttrs>(g, container, {
      allowInvalidContainer: true,
      renderEdgeLabels: false,
      labelDensity: props.graph.nodes.length > 400 ? 0.35 : 1,
      labelGridCellSize: 60,
      defaultEdgeType: "line",
      // reducer 在每次 render 时调用：着色/淡出/描环全部动态算
      nodeReducer: (node, data) => {
        const p = propsRef.current;
        const palette = paletteRef.current;
        const res: Attributes = { ...data };
        const attrs = data as SigmaNodeAttrs;
        const colorKey = attrs.colorKey ?? "todo";
        res.color = resolveColor(colorKey, palette, p.colorMode);
        // 标签策略（万级数据的第一视觉问题）：默认关；只开
        //   hub / 高连接度（≥6）——常态标签源；焦点邻域 / 搜索 / 选中必开。
        // labelDensity 控制 sigma 的标签网格密度，这里 forceLabel 控制资格。
        const degree = g.degree(node);
        const focusId = p.hoverId ?? p.selectedId;
        const inFocusNeighborhood =
          focusId !== null && (node === focusId || g.areNeighbors(node, focusId));
        let labelWorthy =
          attrs.kind === "hub" || degree >= 6 || inFocusNeighborhood;
        // done 淡化（状态着色模式下）：完成态常压标签，聚焦时照常
        const isDoneFade = p.colorMode === "status" && attrs.status === "done";
        if (isDoneFade && !inFocusNeighborhood) labelWorthy = false;
        res.forceLabel = labelWorthy;
        // hover/选中：邻接集合之外的淡出
        let inFocus = true;
        if (focusId) {
          inFocus = inFocusNeighborhood;
          if (!inFocus) {
            res.color = palette.edge;
            res.forceLabel = false;
            res.zIndex = 0;
            res.size = (data.size ?? 4) * 0.6;
            return res;
          }
        }
        res.zIndex = inFocus && focusId ? 1 : 0;
        // 搜索命中 / 选中 / 悬停：强制标签 + highlighted（Sigma 内置描边）
        if (p.searchMatchIds?.has(node) || node === p.selectedId || node === p.hoverId) {
          res.highlighted = true;
          res.forceLabel = true;
        }
        return res;
      },
      edgeReducer: (edge, data) => {
        const p = propsRef.current;
        const palette = paletteRef.current;
        const res: Attributes = { ...data, color: palette.edgeStrong, size: 0.7 };
        const [source, target] = g.extremities(edge);
        const isDep = (data as { kind?: string }).kind === "dep";
        if (isDep) res.color = palette.edgeStrong;
        const focusId = p.hoverId ?? p.selectedId;
        if (focusId && source !== focusId && target !== focusId) {
          res.hidden = true; // 淡出边直接隐藏（万级边下 hidden 比降 alpha 快）
        }
        return res;
      },
    });
    sigmaRef.current = sigma;

    // FA2 worker：物理完全离开主线程
    const fa2 = new FA2Layout(g, { settings: fa2Settings(g.order) });
    fa2.start();
    fa2Ref.current = fa2;
    // 收敛自动停：FA2 会低幅振荡不停机，白烧 CPU。检测量 = 采样节点的坐标
    // **逐点位移和**（不是总量差——总量对万级数据不敏感），阈值按规模缩放。
    // 停机后交互（拖拽）可重启。
    let lastSnapshot = 0;
    const settleTimer = window.setInterval(() => {
      if (!fa2.isRunning()) return;
      let sum = 0;
      let i = 0;
      g.forEachNode((_, attrs) => {
        if (i++ % 5 !== 0) return; // 采样 1/5 节点
        sum += Math.abs(attrs.x - Math.round(attrs.x)) + Math.abs(attrs.y - Math.round(attrs.y));
      });
      // 位移小数部分和稳定 = 整体静止（坐标已冻结在亚像素级）
      if (lastSnapshot > 0 && Math.abs(sum - lastSnapshot) < 1.5) {
        fa2.stop();
      }
      lastSnapshot = sum;
    }, 2500);

    // ── 交互 ──
    sigma.on("enterNode", ({ node }) => propsRef.current.onHover(node));
    sigma.on("leaveNode", () => propsRef.current.onHover(null));
    sigma.on("clickNode", ({ node }) => propsRef.current.onSelect(node));
    sigma.on("clickStage", () => propsRef.current.onSelect(null));
    sigma.on("doubleClickNode", ({ node }) => {
      const attrs = g.getNodeAttributes(node) as SigmaNodeAttrs;
      if (attrs.kind === "hub" && node.startsWith("wf:")) {
        propsRef.current.onOpenHub(node.slice(3));
      }
    });

    // 节点拖拽固定：down 锁定 → move 移动（graphology 坐标）→ up 释放；
    // 拖拽期间暂停 FA2（fixed 节点 FA2 也支持，但暂停更省）
    let draggedId: string | null = null;
    sigma.on("downNode", ({ node }) => {
      draggedId = node;
      fa2.stop();
      g.setNodeAttribute(node, "fixed", true);
      container.style.cursor = "grabbing";
    });
    const onMove = (e: PointerEvent) => {
      if (!draggedId) return;
      const pos = sigma.viewportToGraph({ x: e.clientX, y: e.clientY });
      g.setNodeAttribute(draggedId, "x", pos.x);
      g.setNodeAttribute(draggedId, "y", pos.y);
      // 阻止画布平移
      (sigma.getCamera() as unknown as { animated?: boolean }).animated = false;
    };
    const onUp = () => {
      if (!draggedId) return;
      g.setNodeAttribute(draggedId, "fixed", false);
      draggedId = null;
      container.style.cursor = "default";
      fa2.start(); // 暖启动续跑
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);

    // 主题切换重读配色（切主题不经 React 重渲染）
    const themeObs = new MutationObserver(() => {
      paletteRef.current = readPalette();
      sigma.refresh();
    });
    themeObs.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });

    return () => {
      themeObs.disconnect();
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.clearInterval(settleTimer);
      fa2.kill(); // terminate worker + 释放矩阵内存
      sigma.kill();
      sigmaRef.current = null;
      fa2Ref.current = null;
    };
    // colorMode 变化经 reducer（每次 render 读取 propsRef）自动生效，无需重建
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.graph]);

  // hover/选中/搜索/着色模式变化 → reducer 已读 propsRef，只需 refresh
  useEffect(() => {
    sigmaRef.current?.refresh();
  }, [props.hoverId, props.selectedId, props.searchMatchIds, props.colorMode]);

  return (
    <div
      ref={containerRef}
      className="h-full w-full"
      data-testid="graph-canvas"
      style={{ background: "var(--bg)" }}
    />
  );
}
