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
import {
  computeCameraFit,
  fa2Settings,
  graphBBox,
  resolveOwnerColor,
  resolveStatusColor,
  toGraphologyGraph,
  TAG_ANCHOR_PREFIX,
  type GraphSizeMode,
  type SigmaNodeAttrs,
} from "./graph-adapter";
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

function resolveColor(
  ownerKey: string,
  statusKey: string,
  palette: Palette,
  colorMode: GraphColorMode
): string {
  const cp = { brand: palette.brand, t3: palette.t3, t5: palette.t5, success: palette.success };
  return colorMode === "owner"
    ? resolveOwnerColor(ownerKey, cp)
    : resolveStatusColor(statusKey, cp);
}

export interface GraphCanvasProps {
  graph: BuiltGraph;
  colorMode: GraphColorMode;
  /** 节点大小语义（连接度/耗时）：切换即重建（大小参与 FA2 质量/碰撞，需写回图属性） */
  sizeMode: GraphSizeMode;
  /** 标签 → 同义组键（G6-SYNONYM；键缺失 = 独立组） */
  tagGroups?: Map<string, string>;
  /** owner 注入序（单一事实源，GraphPage 基于 chips 全序计算）：
   *  建图写 ownerKey 与 chips 色点共用，保证图例与节点永远同色 */
  ownerOrder: Map<string, number>;
  /** 「重新布局」触发信号（G4-G6 r2）：+1 启动一轮 FA2 短跑并自动停。
   *  默认静态确定性布局——物理动画只作为手动增强，杜绝大图飞散 */
  relayoutSignal: number;
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
  const relayoutRef = useRef<() => void>(() => {});

  // ── 建图 → Sigma 实例 + FA2 worker（graph 变更时重建；旧实例 kill 释放内存） ──
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const g: Graph<SigmaNodeAttrs> = toGraphologyGraph(
      props.graph,
      props.ownerOrder,
      props.tagGroups,
      props.sizeMode
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
        // 标签锚点恒隐藏（G4-CLUSTER：布局用伪节点，不入视觉/交互）
        if (attrs.kind === "anchor") {
          res.hidden = true;
          return res;
        }
        // 着色双轨：建图时 ownerKey/statusKey 都已固化，按当前模式现场选——
        // 切「按成员/按状态」无需重建图
        res.color = resolveColor(
          attrs.ownerKey,
          attrs.statusKey,
          palette,
          p.colorMode
        );
        // ── 标签色（G3-SIGMA 视觉修订）：专用色阶 + 光晕，见 LABEL_COLORS 注释。
        // hub 高亮半档、常态灰阶、焦点提亮——经自定义 label draw 函数绘制光晕
        const lc = palette.dark ? LABEL_COLORS.dark : LABEL_COLORS.light;
        // 度数用建图值（不含锚点边，锚点边会虚增）
        const degree = attrs.degree ?? 0;
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
        // 标签锚点边恒隐藏（G4-CLUSTER）
        if (source.startsWith(TAG_ANCHOR_PREFIX) || target.startsWith(TAG_ANCHOR_PREFIX)) {
          res.hidden = true;
          return res;
        }
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
    // 钉死归一化参考系：sigma 默认在每次 process()（FA2 每批坐标更新都触发）用
    // 当前 bbox 重基归一化——参考系持续漂移，相机适配无从计算。固定为初始布局
    // bbox 后，归一化映射成为常量，computeCameraFit 才有意义。
    const normExtent = graphBBox(g);
    sigma.setCustomBBox(normExtent);
    // 压测/调试探针（gui 冒烟 harness 用；生产无害）
    (window as unknown as Record<string, unknown>).__graphProbe = {
      graph: g,
      sigma,
      fa2: () => fa2Ref.current,
    };

    // ── 视野自适应（G4-CLUSTER）：把可见节点包围盒动画适配到视口 ──
    // duration=0 时瞬时就位（rAF 节流/隐藏窗口下动画会被冻结，终态必须直接 set）。
    // 跳过非有限坐标——NaN 会把包围盒毒化成 NaN，相机状态随之报废（全屏空白）。
    // 相机态走归一化空间（computeCameraFit）：sigma 相机 x/y/ratio 不在图坐标系，
    // 直写原始坐标会把内容推出视口（小图上全空——坍缩 bug 的根因）。
    // needsFit：视口尺寸未就绪（computeCameraFit 返回 null）时挂起，settle tick
    // 里持续重试——绝不把 Infinity/NaN ratio 写进相机（会把全部节点投影成一点）
    let needsFit = true;
    const fitToContent = (duration = 0) => {
      if (g.order === 0) return;
      let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
      g.forEachNode((_, attrs) => {
        if (!Number.isFinite(attrs.x) || !Number.isFinite(attrs.y)) return;
        if (attrs.x < minX) minX = attrs.x;
        if (attrs.x > maxX) maxX = attrs.x;
        if (attrs.y < minY) minY = attrs.y;
        if (attrs.y > maxY) maxY = attrs.y;
      });
      const size = sigma.getDimensions();
      const target = computeCameraFit(
        { minX, maxX, minY, maxY },
        normExtent,
        size
      );
      if (!target) {
        needsFit = true;
        return;
      }
      needsFit = false;
      if (duration > 0) {
        sigma.getCamera().animate(target, { duration });
      } else {
        sigma.getCamera().setState(target);
      }
    };

    // ── 布局策略（r2 终版）：自动物理动画 + 空间随任务量自适应 ──
    // 布局坐标 ∝ √N（面密度恒定）后，FA2 动力学在任何规模下都能在视野内收敛，
    // 无需降级为静态。收敛自动停机（省 CPU），「重新布局」按钮可手动重跑。
    const fa2 = new FA2Layout(g, { settings: fa2Settings(g.order) });
    fa2.start();
    fa2Ref.current = fa2;
    const timers: number[] = [];

    // 手动重新布局：重跑 FA2（默认 6s 自动停），跑完终态视野适配
    const startRelayout = (runMs = 6000) => {
      fa2.start();
      window.setTimeout(() => {
        if (fa2.isRunning()) fa2.stop();
        fitToContent(400);
      }, runMs);
    };
    relayoutRef.current = () => startRelayout();

    // 收敛自动停 + 坐标消毒：FA2 数值发散（强吸引边 × 碰撞修正下速度爆炸）
    // 会把节点坐标推成 NaN/Inf——WebGL 对含非有限顶点的图元**整体丢弃**，
    // 一个坏节点就清空整帧画布（真机「一闪而过即空白」根因）。
    // 消毒 = 巡检坐标，坏点重置回盘内 + 重启 worker 重读坐标；顺带做收敛检测。
    const LAYOUT_BOUND = 120 * Math.sqrt(Math.max(g.order, 1)) * 4;
    let lastSnapshot = 0;
    const settleTimer = window.setInterval(() => {
      // 视口晚就绪（容器 0×0 起步）的补偿：挂起的 fit 在每个 tick 重试，
      // 直到拿到合法相机态（早退之前检查，FA2 已停也要补）
      if (needsFit) fitToContent();
      if (!fa2.isRunning()) return;
      let sum = 0;
      let i = 0;
      let repaired = 0;
      const golden = 2.399963;
      g.forEachNode((node, attrs) => {
        const bad =
          !Number.isFinite(attrs.x) ||
          !Number.isFinite(attrs.y) ||
          Math.abs(attrs.x) > LAYOUT_BOUND ||
          Math.abs(attrs.y) > LAYOUT_BOUND;
        if (bad) {
          // 坏坐标重置回盘内（黄金角环带，确定性位置）
          const idx = ++repaired;
          const ang = idx * golden;
          const r = 60 + (idx % 11) * 24;
          g.setNodeAttribute(node, "x", Math.cos(ang) * r);
          g.setNodeAttribute(node, "y", Math.sin(ang) * r);
          g.setNodeAttribute(node, "vx", 0);
          g.setNodeAttribute(node, "vy", 0);
        }
        if (i++ % 5 !== 0) return; // 采样 1/5 节点算动能
        sum += Math.abs(attrs.x - Math.round(attrs.x)) + Math.abs(attrs.y - Math.round(attrs.y));
      });
      if (repaired > 0) {
        // 坏点已重置：重启 worker 让矩阵与图坐标同步，再暖跑收敛
        fa2.stop();
        fa2.start();
        fitToContent(400);
        return;
      }
      if (lastSnapshot > 0 && Math.abs(sum - lastSnapshot) < 1.5) {
        fa2.stop();
        fitToContent();
      } else {
        // 布局期间视野跟随：包围盒膨胀时相机不拍原地（节点冲出视野 = 「一闪而过」）
        fitToContent(400);
      }
      lastSnapshot = sum;
    }, 2500);

    // 初始视野适配（布局初值先就位，收敛后再精调）
    const initialFit = window.setTimeout(() => fitToContent(400), 200);
    timers.push(initialFit);

    // ── 交互 ──
    const isBusinessNode = (node: string) =>
      !node.startsWith(TAG_ANCHOR_PREFIX);
    sigma.on("enterNode", ({ node }) => {
      if (isBusinessNode(node)) propsRef.current.onHover(node);
    });
    sigma.on("leaveNode", () => propsRef.current.onHover(null));
    sigma.on("clickNode", ({ node }) => {
      if (isBusinessNode(node)) propsRef.current.onSelect(node);
    });
    sigma.on("clickStage", () => propsRef.current.onSelect(null));
    sigma.on("doubleClickNode", ({ node }) => {
      if (!isBusinessNode(node)) return;
      const attrs = g.getNodeAttributes(node) as SigmaNodeAttrs;
      if (attrs.kind === "hub" && node.startsWith("wf:")) {
        propsRef.current.onOpenHub(node.slice(3));
      }
    });

    // 节点拖拽固定：down 锁定 → move 移动（graphology 坐标）→ up 释放；
    // 拖拽时暂停 FA2（松手续跑），fixed 节点不被物理推走
    let draggedId: string | null = null;
    sigma.on("downNode", ({ node }) => {
      if (!isBusinessNode(node)) return;
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
      fa2.start(); // 暖启动续跑（拖拽后的位置作为新平衡起点）
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
      timers.forEach((t) => window.clearTimeout(t));
      window.clearInterval(settleTimer);
      fa2.kill(); // terminate worker + 释放矩阵内存
      sigma.kill();
      sigmaRef.current = null;
      fa2Ref.current = null;
    };
    // colorMode 变化经 reducer（每次 render 读取 propsRef）自动生效，无需重建。
    // tagGroups/ownerOrder 是建图输入（分扇区初值/锚点/ownerKey）且异步到达
    // （tag_similar_pairs 走嵌入推理，秒级）——必须进依赖，否则真机上扇区布局
    // 永远拿不到同义组（图谱始终以无组圆盘初值运行，与设计不符）；
    // sizeMode 同理（大小参与 FA2 质量/碰撞，切换需重建写回图属性）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.graph, props.tagGroups, props.ownerOrder, props.sizeMode]);

  // 「重新布局」信号：手动触发一轮 FA2 短跑（GraphCanvas 内部已自动停 + 终态适配）
  useEffect(() => {
    if (props.relayoutSignal > 0) relayoutRef.current();
  }, [props.relayoutSignal]);

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
