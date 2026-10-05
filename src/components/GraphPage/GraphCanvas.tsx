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
  /** 主题判定（html.dark），标签色阶按主题取专用值——文本 token 是给 UI 的，
   *  画布标签需要自己的对比度策略（见 LABEL_COLORS 注释） */
  dark: boolean;
}

/**
 * 画布标签专用色阶（G3-SIGMA 视觉修订）：
 * 旧版直接用 --t1/--t2（近黑/近白文本 token）——亮色下黑字压在彩点上生硬，
 * 暗色下默认 labelColor #000 直接消失。
 * 设计原则：
 *  - 常态标签 = 中性灰阶（--t3 系），比正文浅一档——图谱里标签是"路牌"不是正文，
 *    低对比才有"信息在背景里"的层次感（Obsidian 同款取向）
 *  - hub 标签 = 品牌色系，权重高半档（导航锚点）
 *  - 焦点态（hover/选中/搜索）= --t2 提亮一档，白底光晕（亮）/深色光晕（暗）保证
 *    压在任意节点色上都可读
 *  - labelColor 用 color-mode 探测而非硬编码，跟主题即时切换
 */
const LABEL_COLORS = {
  light: {
    normal: "#6b7690", // 亮底上的雾蓝灰（比 --t3 浅、比 --t5 深）
    hub: "#4a5879", // --brand-strong
    focus: "#2b3548", // --t2
    halo: "rgba(255, 255, 255, 0.75)",
  },
  dark: {
    normal: "#9aa5ba", // 暗底上的亮灰蓝（--t3/--t4 之间）
    hub: "#c0cdea", // --brand-strong（暗色亮蓝）
    focus: "#eef1f7", // --t1
    halo: "rgba(20, 22, 29, 0.72)",
  },
} as const;

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
    dark: document.documentElement.classList.contains("dark"),
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
      labelFont: '500 12px ui-sans-serif, system-ui, -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif',
      labelWeight: "500",
      // 标签色由 reducer 写入节点属性（labelColor: { attribute: "labelColor" }）
      // ——主题切换经 palette 重读即时生效，无需重建 sigma
      labelColor: { attribute: "labelColor" as never },
      // 自定义标签绘制：光晕底衬（主题感知）+ 右置文本。压在任意节点色上都可读，
      // 取代 sigma 默认的纯色无衬底（亮底黑字生硬/暗底黑字不可读）
      defaultDrawNodeLabel: (context, data, settings) => {
        if (!data.label) return;
        const palette = paletteRef.current;
        const lc = palette.dark ? LABEL_COLORS.dark : LABEL_COLORS.light;
        const size = settings.labelSize;
        context.font = `${settings.labelWeight} ${size}px ${settings.labelFont}`;
        context.textBaseline = "middle";
        const textW = context.measureText(data.label).width;
        const lx = data.x + data.size + 4;
        // 光晕：与底色同色的半透明圆角衬底——把底下的点/线"抹掉"而不是盖住
        context.fillStyle = lc.halo;
        context.fillRect(lx - 2, data.y - size / 2 - 1, textW + 5, size + 2);
        context.fillStyle =
          (data as unknown as SigmaNodeAttrs).kind === "hub" ? lc.hub : lc.normal;
        context.fillText(data.label, lx, data.y + 1);
      },
      // hover 底板：sigma 默认写死 #FFF 白底——暗色下突兀。几何照抄官方
      // drawDiscNodeHover（圆角胶囊 + 阴影），底色/文字色按主题取。
      defaultDrawNodeHover: (context, data, settings) => {
        if (!data.label) return;
        const palette = paletteRef.current;
        const lc = palette.dark ? LABEL_COLORS.dark : LABEL_COLORS.light;
        const size = settings.labelSize;
        context.font = `${settings.labelWeight} ${size}px ${settings.labelFont}`;
        const PADDING = 4;
        const textW = context.measureText(data.label).width;
        const boxW = Math.round(textW + 8);
        const boxH = Math.round(size + 2 * PADDING);
        const radius = Math.max(data.size, size / 2) + PADDING;
        // 底板阴影（亮色浅影/暗色深影）
        context.shadowOffsetX = 0;
        context.shadowOffsetY = palette.dark ? 2 : 1;
        context.shadowBlur = palette.dark ? 10 : 6;
        context.shadowColor = palette.dark ? "rgba(0,0,0,0.55)" : "rgba(23,32,51,0.18)";
        context.fillStyle = palette.dark ? "#232836" : "#ffffff";
        context.beginPath();
        // 从节点右侧展开的胶囊（与官方一致：含节点的圆头）
        const angle = Math.asin(Math.min(1, boxH / 2 / radius));
        const xDelta = Math.sqrt(Math.abs(radius * radius - (boxH / 2) ** 2));
        context.moveTo(data.x + xDelta, data.y + boxH / 2);
        context.lineTo(data.x + radius + boxW, data.y + boxH / 2);
        context.lineTo(data.x + radius + boxW, data.y - boxH / 2);
        context.lineTo(data.x + xDelta, data.y - boxH / 2);
        context.arc(data.x, data.y, radius, angle, -angle);
        context.closePath();
        context.fill();
        context.shadowOffsetX = 0;
        context.shadowOffsetY = 0;
        context.shadowBlur = 0;
        // 文本（焦点色）
        context.textBaseline = "middle";
        context.fillStyle = lc.focus;
        context.fillText(data.label, data.x + data.size + 6, data.y + 1);
      },
      defaultEdgeType: "line",
      // reducer 在每次 render 时调用：着色/淡出/描环全部动态算
      nodeReducer: (node, data) => {
        const p = propsRef.current;
        const palette = paletteRef.current;
        const res: Attributes = { ...data };
        const attrs = data as SigmaNodeAttrs;
        const colorKey = attrs.colorKey ?? "todo";
        res.color = resolveColor(colorKey, palette, p.colorMode);
        // ── 标签色（G3-SIGMA 视觉修订）：专用色阶 + 光晕，见 LABEL_COLORS 注释。
        // hub 高亮半档、常态灰阶、焦点提亮——经自定义 label draw 函数绘制光晕
        const lc = palette.dark ? LABEL_COLORS.dark : LABEL_COLORS.light;
        const degree = g.degree(node);
        const focusId = p.hoverId ?? p.selectedId;
        const isFocusNode =
          node === p.selectedId || node === p.hoverId ||
          (p.searchMatchIds?.has(node) ?? false);
        const inFocusNeighborhood =
          focusId !== null &&
          (node === focusId || g.areNeighbors(node, focusId) || isFocusNode);
        res.labelColor = isFocusNode || attrs.kind === "hub" ? lc.focus : lc.normal;
        // 标签资格：hub / 高连接度 / 焦点邻域；done 常态压掉
        const isDoneFade = p.colorMode === "status" && attrs.status === "done";
        let labelWorthy =
          attrs.kind === "hub" || degree >= 6 || inFocusNeighborhood;
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
        if (isFocusNode) {
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
