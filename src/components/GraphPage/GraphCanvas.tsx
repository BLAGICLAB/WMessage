// 图谱画布（星云方案）：Three.js 三维场景 + 自研三维力场。
//
// 为什么不用 Sigma：Sigma 是严格二维的，没有相机和投影，拖拽旋转与纵深
// 这两样都做不出来——星云方案的核心恰恰是它们。旧版
// 「Sigma + ForceAtlas2 worker」整套已下线（含首页速览卡共用的 graph-adapter）。
//
// 交互契约与旧版一致：hover 邻接高亮其余淡出、点选详情、拖拽旋转视图、
// 节点拖拽固定、搜索描环、Esc 清除选择、「重新布局」重跑一轮力场。
//
// 布局不在渲染循环里：rAF 每帧只花固定毫秒预算推进力场（时间切片），
// 收敛后停机。大图一次性同步跑完会卡住首帧，万级任务更是会卡十几秒。

import { useEffect, useRef } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { CSS2DObject, CSS2DRenderer } from "three/examples/jsm/renderers/CSS2DRenderer.js";
import { EffectComposer } from "three/examples/jsm/postprocessing/EffectComposer.js";
import { RenderPass } from "three/examples/jsm/postprocessing/RenderPass.js";
import { UnrealBloomPass } from "three/examples/jsm/postprocessing/UnrealBloomPass.js";
import { CloudField, Starfield, hexToRgb01 } from "./NebulaField";
import { Force3D, KINETIC_EPSILON, settle } from "./graph-force3d";
import { buildScene, isHub, labelDegreeFloor, type Scene, type SceneNode } from "./graph-scene";
import { declutterPass, type DeclutterTarget, type Rect } from "./graph-declutter";
import { HUB_COLOR, resolveNodeColor } from "./graph-palette";
import type { BuiltGraph, GraphColorMode } from "./graph-build";
import type { GraphLooseness } from "../../lib/graphPrefs";
import type { GraphEdgeWidth, GraphLabelDensity } from "../../lib/graphPrefs";

/** 连线粗细三档（设置页），单位是名义 CSS 像素：窄带有了真实宽度，
 *  这一档就该真的调宽度。旧的 LineSegments 恒 1px 宽，只能拿 alpha 顶替，
 *  而加色混合会把 alpha 平方衰减——「细」档直接等于看不见。
 *  标准档 1.6px 就是小样的定稿宽度：这三个数就是像素值本身，不再乘任何基准，
 *  近景纵深上量出来就是这个宽度（纵深收细见 EDGE_DEPTH_*）。 */
export const EDGE_WIDTH: Record<GraphEdgeWidth, number> = {
  thin: 1.0,
  standard: 1.6,
  thick: 2.8,
};

/** 窄带按纵深收细：近景更粗、远景更细，密图远景才不会糊成一块板。
 *  参考纵深取小样的机位距离（2.882 × R258 ≈ 744），在这个纵深上系数正好是 1，
 *  于是 EDGE_WIDTH 就是字面意义的屏幕像素；越远越细，但不细过下限——
 *  亚像素的窄带在加色混合下等于看不见，那正是这档设置要避免的事。 */
export const EDGE_DEPTH_REF = 744;
export const EDGE_DEPTH_CLAMP_MIN = 0.5;
export const EDGE_DEPTH_CLAMP_MAX = 1.15;

/** NDC↔像素的换算系数：uRes 宽的视口横跨 2 个 NDC 单位，屏幕 1 像素 = 2/uRes。
 *  它以前是着色器里一个裸字面量，档位表测试够不着——常数改了测试照样全绿。
 *  提成常量插值进着色器，单测才有第二条真值可断言。 */
export const EDGE_NDC_SCALE = 2.0;

/** Bloom 参数。threshold 是「星体发光 / 云团不发光」那条分界线：
 *  星体与星轨芯部越过它起辉，云团粒子恒在它之下。压低了云就开始自己发光。 */
const BLOOM_STRENGTH = 1.35;
const BLOOM_RADIUS = 0.82;
const BLOOM_THRESHOLD = 0.42;

/** 相机：74° 广角（纵深夸张一点，星云才铺得开），近远裁剪面按场景外径给 */
const FOV = 74;
const NEAR_RATIO = 0.004;
const FAR_RATIO = 12;

/** 节点点径系数：世界半径 → 屏幕直径（CSS 像素）的分子。
 *  提成常量插值进着色器，单测才有第二条真值可断言（档位表与着色器脱节过一次）。 */
export const NODE_POINT_SCALE = 2300;

/** 定稿机位（相对布局外径）：视线方向与距离都照抄视觉小样 */
const CAMERA_DIR = { x: -0.372, y: 0.899, z: 2.713 } as const;

/** 机位距离 / 布局外径：星点的屏幕大小 ∝ 1/它（场景层的星点尺寸下限按它换算） */
export const CAMERA_DIST_RATIO = Math.hypot(CAMERA_DIR.x, CAMERA_DIR.y, CAMERA_DIR.z);

const NODE_VERT = /* glsl */ `
  attribute vec3 aColor; attribute float aSize; attribute float aAlpha;
  uniform float uPR;
  varying vec3 vC; varying float vA;
  void main() {
    vC = aColor; vA = aAlpha;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    gl_PointSize = aSize * uPR * (${NODE_POINT_SCALE.toFixed(1)} / max(-mv.z, 1.0));
    gl_Position = projectionMatrix * mv;
  }
`;
const NODE_FRAG = /* glsl */ `
  varying vec3 vC; varying float vA;
  void main() {
    float r = length(gl_PointCoord - 0.5);
    if (r > 0.5) discard;
    float halo = pow(smoothstep(0.5, 0.0, r), 2.4);
    float core = smoothstep(0.17, 0.0, r);
    // 核心往本色里混而不是直接上纯白：加法混合下纯白心会把高连接度节点顶成一团
    // 过曝白斑，色相全丢——那正是「按状态着色」要靠颜色读出来的东西
    vec3 col = vC * halo * 1.30 + mix(vC, vec3(1.0), 0.62) * core * 0.62;
    gl_FragColor = vec4(col, (halo * 0.58 + core * 0.82) * vA);
  }
`;

/** 边：随相机转向的窄带，不是 1 像素线。
 *  LineSegments 恒为 1 像素宽，加色混合下存在感全靠 alpha，而 alpha 又被混和方式
 *  平方衰减，压在 Bloom 阈值下就成了「有数据没画面」。星轨要的是一条有粗细、有软芯的
 *  光路，所以每条边展开成 4 个角点的窄带：横截面 smoothstep 收出软边，芯部亮度
 *  刚好越过阈值起一点辉，边上就散成淡光；远处的边按深度收细，密图远景才不会糊成板。 */
const EDGE_VERT = /* glsl */ `
  attribute vec3 aStart; attribute vec3 aEnd;
  attribute vec3 aColor; attribute float aAlpha; attribute vec2 aCorner;
  uniform vec2 uRes; uniform float uWidth;
  varying vec3 vC; varying float vA; varying float vS;
  void main() {
    vC = aColor; vA = aAlpha; vS = aCorner.x;
    vec4 cs = projectionMatrix * modelViewMatrix * vec4(aStart, 1.0);
    vec4 ce = projectionMatrix * modelViewMatrix * vec4(aEnd, 1.0);
    // 端点跑到相机背后时透视除法会把窄带拉成横贯全屏的巨块，直接塌掉
    if (cs.w <= 0.0 || ce.w <= 0.0) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
    vec2 dir = (ce.xy / ce.w - cs.xy / cs.w) * uRes;
    float len = length(dir);
    dir = len > 1e-4 ? dir / len : vec2(1.0, 0.0);
    // ×2 是 NDC↔像素的换算：uRes 宽的视口横跨 2 个 NDC 单位，
    //  漏掉这个系数，uWidth 标称的像素宽会只剩一半（实测三档全落进亚像素）
    vec2 nrm = vec2(-dir.y, dir.x) * ${EDGE_NDC_SCALE.toFixed(1)} / uRes;
    float depth = -(modelViewMatrix * vec4(aStart, 1.0)).z;
    vec4 p = mix(cs, ce, aCorner.y);
    p.xy += nrm * (aCorner.x * uWidth * 0.5 * clamp(${EDGE_DEPTH_REF.toFixed(1)} / max(depth, 1.0), ${EDGE_DEPTH_CLAMP_MIN}, ${EDGE_DEPTH_CLAMP_MAX})) * p.w;
    gl_Position = p;
  }
`;
const EDGE_FRAG = /* glsl */ `
  varying vec3 vC; varying float vA; varying float vS;
  void main() {
    float w = smoothstep(0.0, 1.0, 1.0 - abs(vS));
    gl_FragColor = vec4(vC * vA * w, w * vA);
  }
`;

/** 每条边的角点数（窄带 = 2 个三角形 = 4 个角） */
const EDGE_SEG = 4;

/** 星轨材质。档位值从 EDGE_WIDTH 直接进 uWidth，着色器里只有 uWidth 一个宽度来源——
 *  单测靠它够到 uniform 接线（曾出现过「档位表是 1.6、着色器另乘一个基准」的整档错位）。 */
export function createEdgeMaterial(pref: GraphEdgeWidth): THREE.ShaderMaterial {
  return new THREE.ShaderMaterial({
    transparent: true,
    depthWrite: false,
    side: THREE.DoubleSide,
    blending: THREE.AdditiveBlending,
    uniforms: {
      uRes: { value: new THREE.Vector2(1, 1) },
      uWidth: { value: EDGE_WIDTH[pref] },
    },
    vertexShader: EDGE_VERT,
    fragmentShader: EDGE_FRAG,
  });
}

/** 外观刷新写完颜色/尺寸后必须标脏，否则画面停在上一帧。
 *  这里曾经只标了边、漏了星体：收敛之后 pushGeometry 不再被调用，切着色维度
 *  就成了「连线变色、星体不变」。两边的规则收成一个函数，再漏时单测直接红。 */
export function markAppearanceDirty(
  nodeGeo: THREE.BufferGeometry,
  edgeGeo: THREE.BufferGeometry,
): void {
  nodeGeo.attributes.aColor.needsUpdate = true;
  nodeGeo.attributes.aSize.needsUpdate = true;
  edgeGeo.attributes.aColor.needsUpdate = true;
  edgeGeo.attributes.aAlpha.needsUpdate = true;
}

export interface GraphCanvasProps {
  graph: BuiltGraph;
  colorMode: GraphColorMode;
  /** 标签密度（少=仅焦点 / 标准 / 多=全部） */
  labelDensity: GraphLabelDensity;
  /** 连线粗细 */
  edgeWidth: GraphEdgeWidth;
  /** 打开时自动跑布局动画（关 = 直接给静态构图，「重新布局」仍可手动跑） */
  autoLayout: boolean;
  /** 布局松散度：切换即重建 */
  looseness: GraphLooseness;
  /** 标签 → 同义组键（键缺失 = 独立组） */
  tagGroups?: Map<string, string>;
  /** 成员注入序（与 chips 色点共用同一事实源） */
  ownerOrder: Map<string, number>;
  /** 标签注入序（与 chips 色点共用同一事实源） */
  tagOrder: Map<string, number>;
  /** 成员语义键 → 显示名（锚点铭牌要印真名，不是 owner:0） */
  ownerNames?: Map<string, string>;
  /** 「重新布局」触发信号：+1 重新暖机跑一轮力场 */
  relayoutSignal: number;
  selectedId: string | null;
  hoverId: string | null;
  searchMatchIds: Set<string> | null;
  onHover: (id: string | null) => void;
  onSelect: (id: string | null) => void;
  /** 画布外可点的区域（详情面板、图例等）：铭牌避让要把它们当障碍 */
  obstacleRects: () => Rect[];
}

interface Plaque {
  el: HTMLDivElement;
  /** 承载避让偏移的里层（外层 transform 由 CSS2DRenderer 每帧覆写） */
  off: HTMLSpanElement;
  obj: CSS2DObject;
  w: number;
  h: number;
  dx: number;
  dy: number;
  /** 找不着落点：压透明度隐藏，而不是置 visible=false——
   *  置 false 之后它就永远退出摆位，下个机位明明空出来了也回不来 */
  cull: boolean;
  want: boolean;
  lift: number;
}

const STEP_BUDGET_MS = 10;

/** separate() 是 O(N²)，超过这个规模就不跑了：万级任务宁可在几处小重叠上让步 */
const SEPARATE_MAX_N = 1200;
/** 补间距时每颗节点的半径倍率（nodeRadius 已经含 √ 度数压缩） */
const SEPARATE_GAP = 2.2;
/** 铭牌避让每帧的预算。摆位是 O(M²)：3200 块实测 15ms、12800 块 68ms，
 *  一帧 16ms 根本放不下。预算耗尽后剩下的铭牌先隐一帧，下一帧接着摆。 */
const DECLUTTER_BUDGET_MS = 4;

/** 上一次的构图与机位（模块级，跨重建与卸载都在）。
 *  改一条依赖只是换了几条边：节点集合与外径都没变。这时候把整场拆掉重建，等于
 *  把用户刚摆好的视角弹回定稿机位、再让整片星云从种子重排一遍——数据没大动，
 *  画面却整个重来。 */
interface LayoutCache {
  ids: string[];
  radius: number;
  pos: Float32Array;
  camera: [number, number, number];
  target: [number, number, number];
}
let layoutCache: LayoutCache | null = null;

/** 能不能接着上次的画面走：必须是同一批节点（逐位一致）与同一外径。
 *  换了松散度、增删了任务、动了过滤器——构图含义已经变了，重新来。 */
export function canReuseLayout(
  cache: LayoutCache | null,
  ids: string[],
  radius: number,
): cache is LayoutCache {
  if (!cache || cache.ids.length !== ids.length) return false;
  // R = k·√N，同一份输入每次算出的是同一个数，不需要容差
  if (cache.radius !== radius) return false;
  return cache.ids.every((id, i) => id === ids[i]);
}

export default function GraphCanvas(props: GraphCanvasProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const propsRef = useRef(props);
  propsRef.current = props;

  // ── 建图 → 场景 + 力场 + Three 实例（场景输入变更时重建） ──
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const p = propsRef.current;
    const scene: Scene = buildScene(p.graph, {
      ownerOrder: p.ownerOrder,
      tagOrder: p.tagOrder,
      tagGroups: p.tagGroups,
      ownerNames: p.ownerNames,
      looseness: p.looseness,
      now: Date.now(),
    });
    const N = scene.nodes.length;
    const E = scene.edges.length;
    const force = new Force3D(scene);

    // ── 力场：rAF 时间切片推进 ──
    let settled = false;
    let running = false;
    const pos = force.pos;
    /** 唤醒力场。所有「让它继续跑」的路径都必须走这里——曾经拖拽松手那条漏了，
     *  结果节点停在松手处再也不动，注释却写着「从新位置继续平衡」。
     *  停机只有一处（收敛），恢复却有好几条，漏一条就是一块死区。 */
    const resumeForce = () => {
      force.reheat();
      settled = false;
      running = true;
    };
    const labelFloor = labelDegreeFloor(scene.nodes);

    const renderer = new THREE.WebGLRenderer({
      antialias: true,
      powerPreference: "high-performance",
    });
    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
    renderer.setPixelRatio(pixelRatio);
    // 纯黑真空：没有星雾底色，背景就是宇宙本身
    renderer.setClearColor(0x000000, 1);
    // 不接 OutputPass、也不开色调映射：着色器直出的就是设计稿里的原始 sRGB 色值，
    // 再过一层 sRGB 转换等于把整套调色板整体抬亮一档
    renderer.toneMapping = THREE.NoToneMapping;
    renderer.outputColorSpace = THREE.LinearSRGBColorSpace;
    container.appendChild(renderer.domElement);

    const labelHost = document.createElement("div");
    labelHost.className = "graph-labels";
    container.appendChild(labelHost);
    const labelRenderer = new CSS2DRenderer({ element: labelHost });

    const world = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(FOV, 1, 1, 6000);
    const R = scene.radius;
    // 视觉小样的定稿机位：(-96, 232, 700) / R 258 = 2.882R。视线方向同比例保留，
    // 长度也照抄——这条距离直接决定星点的屏幕大小（点径 ∝ 1/距离），
    // 凑近了就等于把粒子相对云胀一圈，云一叠就自己发起光来
    camera.position.set(R * CAMERA_DIR.x, R * CAMERA_DIR.y, R * CAMERA_DIR.z);
    camera.near = R * NEAR_RATIO;
    camera.far = R * FAR_RATIO;

    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.dampingFactor = 0.06;
    controls.rotateSpeed = 0.62;
    controls.zoomSpeed = 0.72;
    controls.minDistance = R * 0.9;
    controls.maxDistance = R * 7.5;
    // 静止时极缓自转：图谱是「活着」的，但不抢注意力
    controls.autoRotate = true;
    controls.autoRotateSpeed = 0.22;
    if (
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      controls.autoRotate = false;
    }

    // 同一批节点、同一外径：接着上次的坐标与机位走，别把用户正在看的视角弹回去
    const cache = layoutCache;
    if (canReuseLayout(cache, scene.nodes.map((n) => n.id), R)) {
      for (let i = 0; i < N; i++) {
        pos[i * 3] = cache.pos[i * 3];
        pos[i * 3 + 1] = cache.pos[i * 3 + 1];
        pos[i * 3 + 2] = cache.pos[i * 3 + 2];
      }
      camera.position.set(...cache.camera);
      controls.target.set(...cache.target);
      controls.update();
    }

    const stars = new Starfield({ sceneRadius: scene.radius, pixelRatio });
    stars.addTo(world);
    // 云团挂在结构锚点上：一团正好罩一组同类星体，颜色才有得可反射
    const clouds = new CloudField({
      anchors: scene.nodes.map((n, i) => (n.kind === "hub" ? i : -1)).filter((i) => i >= 0),
      samplable: scene.nodes.map((n, i) => (n.kind === "task" ? i : -1)).filter((i) => i >= 0),
      sceneRadius: scene.radius,
      pixelRatio,
    });
    clouds.addTo(world);
    // 云色只在调色板/筛选变过时重算；跟随锚点的位移则每帧更新
    let cloudsDirty = true;

    // ── 节点点云 ──
    const nodePos = new Float32Array(N * 3);
    const nodeColor = new Float32Array(N * 3);
    const nodeSize = new Float32Array(N);
    const nodeAlpha = new Float32Array(N);
    // 注意力/筛选层算出的基础亮度。nodeAlpha 是它乘上深度衰减后的结果——
    // 相机一直在自转，远近关系每帧都在变，两者得分开存
    const alphaBase = new Float32Array(N);
    const nodeGeo = new THREE.BufferGeometry();
    nodeGeo.setAttribute("position", new THREE.BufferAttribute(nodePos, 3));
    nodeGeo.setAttribute("aColor", new THREE.BufferAttribute(nodeColor, 3));
    nodeGeo.setAttribute("aSize", new THREE.BufferAttribute(nodeSize, 1));
    nodeGeo.setAttribute("aAlpha", new THREE.BufferAttribute(nodeAlpha, 1));
    const nodeMat = new THREE.ShaderMaterial({
      transparent: true,
      depthWrite: false,
      blending: THREE.AdditiveBlending,
      uniforms: { uPR: { value: pixelRatio } },
      vertexShader: NODE_VERT,
      fragmentShader: NODE_FRAG,
    });
    const nodePoints = new THREE.Points(nodeGeo, nodeMat);
    world.add(nodePoints);

    // ── 边：星轨窄带 ──
    const edgeStart = new Float32Array(E * EDGE_SEG * 3);
    const edgeEnd = new Float32Array(E * EDGE_SEG * 3);
    const edgeColor = new Float32Array(E * EDGE_SEG * 3);
    const edgeAlpha = new Float32Array(E * EDGE_SEG);
    const edgeCorner = new Float32Array(E * EDGE_SEG * 2);
    const edgeIdx = new Uint32Array(E * 6);
    for (let e = 0; e < E; e++) {
      const v = e * EDGE_SEG;
      const o = e * 6;
      // 角点按 (-1,0) (+1,0) (-1,1) (+1,1) 铺开，索引连成两张同向三角形
      edgeIdx[o] = v; edgeIdx[o + 1] = v + 1; edgeIdx[o + 2] = v + 3;
      edgeIdx[o + 3] = v; edgeIdx[o + 4] = v + 3; edgeIdx[o + 5] = v + 2;
      for (let c = 0; c < EDGE_SEG; c++) {
        edgeCorner[(v + c) * 2] = c & 1 ? 1 : -1;
        edgeCorner[(v + c) * 2 + 1] = c >> 1;
      }
    }
    const edgeGeo = new THREE.BufferGeometry();
    edgeGeo.setAttribute("aStart", new THREE.BufferAttribute(edgeStart, 3));
    edgeGeo.setAttribute("aEnd", new THREE.BufferAttribute(edgeEnd, 3));
    edgeGeo.setAttribute("aColor", new THREE.BufferAttribute(edgeColor, 3));
    edgeGeo.setAttribute("aAlpha", new THREE.BufferAttribute(edgeAlpha, 1));
    edgeGeo.setAttribute("aCorner", new THREE.BufferAttribute(edgeCorner, 2));
    edgeGeo.setIndex(new THREE.BufferAttribute(edgeIdx, 1));
    // 顶点是着色器里才展开的，包围球只能给个假值，否则算出来是 NaN
    edgeGeo.boundingSphere = new THREE.Sphere(new THREE.Vector3(), 1e6);
    const edgeMat = createEdgeMaterial(props.edgeWidth);
    const edgeLines = new THREE.Mesh(edgeGeo, edgeMat);
    edgeLines.frustumCulled = false;
    world.add(edgeLines);

    const composer = new EffectComposer(renderer);
    composer.addPass(new RenderPass(world, camera));
    const bloom = new UnrealBloomPass(
      new THREE.Vector2(1, 1),
      BLOOM_STRENGTH,
      BLOOM_RADIUS,
      BLOOM_THRESHOLD,
    );
    composer.addPass(bloom);

    // ── 选中环：全场唯一允许「动」的结构件 ──
    const ringMat = new THREE.MeshBasicMaterial({
      color: 0x8ff2e0,
      transparent: true,
      opacity: 0,
      side: THREE.DoubleSide,
      depthWrite: false,
      blending: THREE.AdditiveBlending,
    });
    const ring = new THREE.Mesh(new THREE.RingGeometry(1, 1.16, 72), ringMat);
    ring.visible = false;
    world.add(ring);

    // ── 铭牌（CSS2D 承载文字 + 屏幕空间避让） ──
    const plaques: Plaque[] = scene.nodes.map((node) => {
      const el = document.createElement("div");
      el.className = `graph-plaque${node.kind === "task" ? "" : " is-hub"}`;
      // 偏移挂在里层：CSS2DRenderer 每帧覆写外层 transform（定位用），
      // 避让位移再写在外层等于两者相乘，铭牌实际会跑出避让模型算出的落点
      const off = document.createElement("span");
      off.className = "graph-plaque-off";
      off.innerHTML = node.kind === "task"
        ? `${node.label}<span class="graph-plaque-sub">${node.degree}</span>`
        : `<span class="graph-plaque-kind">${hubRole(node)}</span>${node.label}`;
      el.appendChild(off);
      el.style.opacity = "0";
      const obj = new CSS2DObject(el);
      world.add(obj);
      return {
        el,
        off,
        obj,
        w: 0,
        h: 0,
        dx: 0,
        dy: 0,
        cull: false,
        want: false,
        lift: node.kind === "task" ? -16 : -22,
      };
    });

    // 落位顺序：锚点先占（它们是骨架，路牌比任务名重要），任务按连接度降序。
    // 谁重要谁先占位，撞上了才让
    const order = scene.nodes
      .map((node, i) => ({ node, i }))
      .sort((a, b) =>
        (a.node.kind === "task" ? 1 : 0) - (b.node.kind === "task" ? 1 : 0) ||
        b.node.degree - a.node.degree,
      )
      .map((x) => x.i);

    // ── 邻接表：hover/选中高亮、拾取都用它 ──
    const adjacency: Array<Set<number>> = scene.nodes.map(() => new Set());
    scene.edges.forEach((e) => {
      adjacency[e.a].add(e.b);
      adjacency[e.b].add(e.a);
    });

    // ── 语义键 → 颜色（建图时固化，切着色模式只重算颜色，不动布局） ──
    const rgbOf = (hex: string) => hexToRgb01(hex);
    const HUB_RGB = rgbOf(HUB_COLOR);
    const baseColor: Array<[number, number, number]> = scene.nodes.map((node) =>
      node.kind === "task"
        ? rgbOf(resolveNodeColor("status", node))
        : HUB_RGB,
    );
    const baseSize = new Float32Array(N);
    scene.nodes.forEach((node, i) => {
      baseSize[i] = node.kind === "task" ? node.radius : node.radius * 1.15;
    });

    const writeColors = (mode: GraphColorMode) => {
      scene.nodes.forEach((node, i) => {
        const c = node.kind === "task" ? rgbOf(resolveNodeColor(mode, node)) : HUB_RGB;
        baseColor[i] = c;
      });
    };

    // ── 注意力（选中优先，无选中才用 hover）：全场只压暗一次 ──
    const focusIndex = () => {
      const p = propsRef.current;
      if (p.selectedId) return scene.indexOf.get(p.selectedId) ?? -1;
      if (p.hoverId) return scene.indexOf.get(p.hoverId) ?? -1;
      return -1;
    };

    // 搜索命中节点（≤18 个时全体亮铭牌，超过只留计数——一片亮字等于没亮）
    const searchHits = () => {
      const ids = propsRef.current.searchMatchIds;
      if (!ids || ids.size === 0) return [] as number[];
      return [...ids]
        .map((id) => scene.indexOf.get(id))
        .filter((i): i is number => i !== undefined);
    };

    const refreshAppearance = () => {
      const p = propsRef.current;
      writeColors(p.colorMode);
      const focus = focusIndex();
      const near = new Set<number>();
      if (focus >= 0) {
        near.add(focus);
        adjacency[focus].forEach((j) => near.add(j));
      }
      const hits = new Set(searchHits());
      const inFocus = (i: number) => focus < 0 || near.has(i);

      for (let i = 0; i < N; i++) {
        const isFocus = i === focus;
        const isHit = hits.has(i);
        // 别把非注意力节点压成背景尘：压太狠整片星云看着像被清空了
        const lit = inFocus(i) && scene.nodes[i].kind === "task";
        alphaBase[i] = lit ? (isFocus || isHit ? 1 : 0.85) : 0.26;
        nodeSize[i] = baseSize[i] * (isFocus ? 1.5 : isHit ? 1.2 : 1);
        const c = baseColor[i];
        nodeColor[i * 3] = c[0];
        nodeColor[i * 3 + 1] = c[1];
        nodeColor[i * 3 + 2] = c[2];
      }
      // 锚点：结构骨架常驻，不参与筛选淡出（它不是检索对象）
      for (let i = 0; i < N; i++) {
        if (scene.nodes[i].kind === "hub") {
          alphaBase[i] = focus < 0 || near.has(i) ? 0.5 : 0.22;
        }
      }

      // 边：只保留注意力邻域内的，其余退成尘。
      // 加色混合下 alpha 会被混和方式平方衰减，所以这里的数值看着虚高是刻意的：
      // 平方之后语义边才刚好爬到 Bloom 阈值起辉，弱边仍留在阈值下当淡光路
      edgeMat.uniforms.uWidth.value = EDGE_WIDTH[p.edgeWidth];
      for (let e = 0; e < E; e++) {
        const { a, b } = scene.edges[e];
        const on = focus < 0 || a === focus || b === focus ||
          (near.has(a) && near.has(b));
        // 只压 alpha 不换色：星轨要的是「这束光从哪颗星发出来」，
        // 换成中性灰整片就退化成一张灰网，分不出连线归属
        const alpha = on ? (focus < 0 ? 0.72 : 0.95) : 0.2;
        const c = baseColor[a];
        for (let k = 0; k < EDGE_SEG; k++) {
          const v = (e * EDGE_SEG + k) * 3;
          edgeColor[v] = c[0];
          edgeColor[v + 1] = c[1];
          edgeColor[v + 2] = c[2];
          edgeAlpha[e * EDGE_SEG + k] = alpha;
        }
      }
      markAppearanceDirty(nodeGeo, edgeGeo);
      cloudsDirty = true;

      // 铭牌资格：锚点常驻；任务按密度档位 + 注意力
      const dense = p.labelDensity === "dense";
      const sparse = p.labelDensity === "sparse";
      // hits 就是 searchHits() 那一份，同一次刷新里搜命中不会变，不必再算一遍
      const showAllHits = hits.size > 0 && hits.size <= 18;
      // 选中态只放行焦点 + 最重的 12 个邻居，否则节点一选中就是一片字墙
      const topNeighbors = new Set<number>();
      if (focus >= 0) {
        topNeighbors.add(focus);
        [...adjacency[focus]]
          .sort((x, y) => scene.nodes[y].degree - scene.nodes[x].degree)
          .slice(0, 12)
          .forEach((j) => topNeighbors.add(j));
      }
      for (let i = 0; i < N; i++) {
        const node = scene.nodes[i];
        let want: boolean;
        if (node.kind === "hub") {
          want = true;
        } else if (showAllHits && hits.has(i)) {
          want = true;
        } else if (focus >= 0) {
          want = topNeighbors.has(i);
        } else {
          want = dense || (!sparse && node.degree >= labelFloor);
        }
        if (want && focus >= 0 && !inFocus(i) && !showAllHits) want = false;
        plaques[i].want = want;
        plaques[i].obj.visible = want;
        plaques[i].el.style.opacity = want ? "1" : "0";
      }
      ringMat.opacity = 0.85;
    };

    refreshAppearance();

    // ── 视口 ──
    let width = 0;
    let height = 0;
    const resize = () => {
      const w = container.clientWidth;
      const h = container.clientHeight;
      if (w < 2 || h < 2) return;
      width = w;
      height = h;
      // 第三参必须留默认 true：置 false 时 three 只改绘图缓冲、不写 style，
      // canvas 就按「缓冲像素」去布局（= CSS 尺寸 × devicePixelRatio），
      // 直接撑成容器的两倍再被 overflow 裁掉左上一角——看着像没居中
      renderer.setSize(w, h);
      composer.setSize(w, h);
      labelRenderer.setSize(w, h);
      edgeMat.uniforms.uRes.value.set(w, h);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(container);

    // ── 拾取：屏幕空间最近点（点云没有实体可射线求交，
    //    而且铭牌避让本来就要每个节点投影一次，顺手复用） ──
    const proj = new THREE.Vector3();
    let screen: Float32Array = new Float32Array(N * 3);
    const projectAll = () => {
      for (let i = 0; i < N; i++) {
        proj.set(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]).project(camera);
        screen[i * 3] = (proj.x * 0.5 + 0.5) * width;
        screen[i * 3 + 1] = (-proj.y * 0.5 + 0.5) * height;
        screen[i * 3 + 2] = proj.z;
      }
    };
    const pick = (mx: number, my: number): number => {
      let best = -1;
      let bestD = 22 * 22;
      let bestZ = Infinity;
      for (let i = 0; i < N; i++) {
        if (isHub(scene.nodes[i].id)) continue;
        if (screen[i * 3 + 2] > 1) continue; // 相机背后
        const dx = screen[i * 3] - mx;
        const dy = screen[i * 3 + 1] - my;
        const d = dx * dx + dy * dy;
        if (d > bestD) continue;
        // 屏幕上更近优先；距离几乎相同时取离相机更近的（视觉上在前面那个）
        if (d > bestD * 0.7 && screen[i * 3 + 2] > bestZ) continue;
        best = i;
        bestD = Math.min(bestD, d);
        bestZ = screen[i * 3 + 2];
      }
      return best;
    };

    const idAt = (i: number) => (i >= 0 ? scene.nodes[i].id : null);

    // ── 指针事件 ──
    const ray = new THREE.Raycaster();
    const grabNormal = new THREE.Vector3();
    const grabHit = new THREE.Vector3();
    const grabOffset = new THREE.Vector3();
    const grabPlane = new THREE.Plane();
    const ndc = new THREE.Vector2();
    const lastPointer = { clientX: 0, clientY: 0 };
    let dragging = -1;
    const ndcOf = (p: { clientX: number; clientY: number }, rect: DOMRect) => {
      ndc.set(
        ((p.clientX - rect.left) / rect.width) * 2 - 1,
        -((p.clientY - rect.top) / rect.height) * 2 + 1,
      );
      return ndc;
    };

    const onPointerMove = (e: PointerEvent) => {
      lastPointer.clientX = e.clientX;
      lastPointer.clientY = e.clientY;
      const rect = renderer.domElement.getBoundingClientRect();
      // 当帧重新拾取：hover 状态可能来自上一帧的相机位置，直接拿旧命中会错
      propsRef.current.onHover(idAt(pick(e.clientX - rect.left, e.clientY - rect.top)));
    };
    const onPointerDown = (e: PointerEvent) => {
      if (e.button !== 0) return;
      const rect = renderer.domElement.getBoundingClientRect();
      const i = pick(e.clientX - rect.left, e.clientY - rect.top);
      if (i < 0) return;
      // 按在业务节点上 = 固定它（拖走），否则交给 OrbitControls 转视角
      controls.enabled = false;
      dragging = i;
      camera.getWorldDirection(grabNormal);
      grabPlane.setFromNormalAndCoplanarPoint(
        grabNormal,
        new THREE.Vector3(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]),
      );
      ray.setFromCamera(ndcOf(e, rect), camera);
      ray.ray.intersectPlane(grabPlane, grabHit);
      grabOffset.set(
        pos[i * 3] - grabHit.x,
        pos[i * 3 + 1] - grabHit.y,
        pos[i * 3 + 2] - grabHit.z,
      );
    };
    const onPointerUp = () => {
      if (dragging < 0) return;
      controls.enabled = true;
      dragging = -1;
      // 拖拽是在力场之外改的坐标，松手得让它重新参与平衡，否则全场就此定格
      resumeForce();
    };
    const onClick = (e: MouseEvent) => {
      const rect = renderer.domElement.getBoundingClientRect();
      const i = pick(e.clientX - rect.left, e.clientY - rect.top);
      propsRef.current.onSelect(idAt(i));
    };
    const onLeave = () => propsRef.current.onHover(null);

    const el = renderer.domElement;
    el.addEventListener("pointermove", onPointerMove);
    el.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("pointerup", onPointerUp);
    el.addEventListener("click", onClick);
    el.addEventListener("pointerleave", onLeave);

    const pushGeometry = () => {
      for (let i = 0; i < N; i++) {
        nodePos[i * 3] = pos[i * 3];
        nodePos[i * 3 + 1] = pos[i * 3 + 1];
        nodePos[i * 3 + 2] = pos[i * 3 + 2];
        plaques[i].obj.position.set(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]);
      }
      for (let e = 0; e < E; e++) {
        const { a, b } = scene.edges[e];
        for (let k = 0; k < EDGE_SEG; k++) {
          const v = (e * EDGE_SEG + k) * 3;
          edgeStart[v] = pos[a * 3];
          edgeStart[v + 1] = pos[a * 3 + 1];
          edgeStart[v + 2] = pos[a * 3 + 2];
          edgeEnd[v] = pos[b * 3];
          edgeEnd[v + 1] = pos[b * 3 + 1];
          edgeEnd[v + 2] = pos[b * 3 + 2];
        }
      }
      nodeGeo.attributes.position.needsUpdate = true;
      nodeGeo.attributes.aColor.needsUpdate = true;
      nodeGeo.attributes.aSize.needsUpdate = true;
      applyDepthFade();
      nodeGeo.attributes.aAlpha.needsUpdate = true;
      edgeGeo.attributes.aStart.needsUpdate = true;
      edgeGeo.attributes.aEnd.needsUpdate = true;
      nodeGeo.computeBoundingSphere();
    };

    /**
     * 深度衰减：远处退进黑场，深度感靠亮度差而不是描边。
     *
     * 少了这一层，远半边星云和近半边一样亮。加色混合下几百个亮点在同一块
     * 像素上逐粒累加，远侧不压暗就会直接顶过 Bloom 阈值糊成一片过曝白斑——
     * 那是「云自己在发光」的观感，正好把星云的分层拆了。
     * 系数折算自小样（260 / 620 @ R=258，相机 2.87R），按 R 等比放大。
     */
    const applyDepthFade = () => {
      const cam = camera.position;
      for (let i = 0; i < N; i++) {
        const dx = pos[i * 3] - cam.x;
        const dy = pos[i * 3 + 1] - cam.y;
        const dz = pos[i * 3 + 2] - cam.z;
        const d = Math.sqrt(dx * dx + dy * dy + dz * dz);
        const fade = Math.min(Math.max(1.4 - (d - R) / (R * 2.403), 0.46), 1);
        nodeAlpha[i] = alphaBase[i] * fade;
      }
    };

    /**
     * 收敛收尾，顺序不能换：先 separate 再 normalize。
     *
     * separate 按绝对间距把还贴在一起的两点推开，normalize 是把全场整体乘一个
     * 系数去命中 R——反过来的话，推开的距离会被随后的缩放再改一次，最后外径
     * 就不是 R 了。开不开布局动画走的是同一个函数，否则同一份数据开和关
     * 会长出两张不同的图。
     */
    const finishLayout = () => {
      // 小图补一遍最小间距：收敛后仍贴在一起的两个点，光点和铭牌会叠成一坨
      if (N <= SEPARATE_MAX_N) force.separate(nodeRadiusOf(scene) * SEPARATE_GAP);
      force.normalize(R);
    };

    const restLayout = () => {
      finishLayout();
      resumeForce();
    };
    if (p.autoLayout) {
      running = true;
    } else {
      // 静态构图：一次跑到底。大图照样走时间切片，只是每帧预算给足
      settle(force, { maxSteps: 700 });
      finishLayout();
      settled = true;
      running = false;
    }

    // THREE.Clock 已废弃且只为拿一个 elapsed，自己记起点即可
    const startedAt = performance.now();
    let raf = 0;
    const obstacles: Rect[] = [];

    const declutter = () => {
      obstacles.length = 0;
      for (const o of propsRef.current.obstacleRects()) obstacles.push(o);
      const wanted: Plaque[] = [];
      const targets: DeclutterTarget[] = [];
      for (const i of order) {
        const pl = plaques[i];
        if (!pl.want) continue;
        if (!pl.w) {
          // 首帧元素还没被 CSS2DRenderer 挂进 DOM，量不到尺寸就等下一帧
          pl.w = pl.el.offsetWidth;
          pl.h = pl.el.offsetHeight;
          if (!pl.w) continue;
        }
        wanted.push(pl);
        targets.push({
          cx: screen[i * 3],
          cy: screen[i * 3 + 1],
          behind: screen[i * 3 + 2] > 1,
          w: pl.w,
          h: pl.h,
          lift: pl.lift,
          dx: pl.dx,
          dy: pl.dy,
        });
      }
      const results = declutterPass(targets, obstacles, width, height, {
        budgetMs: DECLUTTER_BUDGET_MS,
      });
      for (let k = 0; k < wanted.length; k++) {
        const pl = wanted[k];
        const r = results[k];
        pl.cull = r.culled;
        pl.dx = r.dx;
        pl.dy = r.dy;
        if (r.culled) continue;
        pl.off.style.setProperty("--dx", `${r.dx.toFixed(0)}px`);
        pl.off.style.setProperty("--dy", `${r.dy.toFixed(0)}px`);
        pl.el.style.opacity = "1";
      }
    };

    const tick = () => {
      raf = requestAnimationFrame(tick);
      const t = (performance.now() - startedAt) / 1000;

      if (running && !settled) {
        // 时间切片：一帧只花固定预算跑力场，大图也不会卡住主线程
        const deadline = performance.now() + STEP_BUDGET_MS;
        let steps = 0;
        while (performance.now() < deadline && steps < 400) {
          force.step();
          steps++;
          if (force.kinetic() < KINETIC_EPSILON) {
            settled = true;
            break;
          }
        }
        if (settled) {
          finishLayout();
          running = false;
          refreshAppearance();
        }
        pushGeometry();
      }

      if (dragging >= 0) {
        // 拖拽固定：节点跟着指针走，跑在力场之外，松手后从新位置继续平衡
        const rect = el.getBoundingClientRect();
        ray.setFromCamera(ndcOf(lastPointer, rect), camera);
        if (ray.ray.intersectPlane(grabPlane, grabHit)) {
          pos[dragging * 3] = grabHit.x + grabOffset.x;
          pos[dragging * 3 + 1] = grabHit.y + grabOffset.y;
          pos[dragging * 3 + 2] = grabHit.z + grabOffset.z;
          pushGeometry();
        }
      }

      controls.update();
      applyDepthFade();
      nodeGeo.attributes.aAlpha.needsUpdate = true;
      projectAll();

      const sel = propsRef.current.selectedId;
      const selIdx = sel ? scene.indexOf.get(sel) ?? -1 : -1;
      if (selIdx >= 0) {
        ring.visible = true;
        ring.position.set(pos[selIdx * 3], pos[selIdx * 3 + 1], pos[selIdx * 3 + 2]);
        ring.lookAt(camera.position);
        const s = scene.nodes[selIdx].radius * 2.4;
        ring.scale.setScalar(s);
      } else {
        ring.visible = false;
      }

      // 铭牌：压透明度隐藏，不是 visible=false（否则永远退出摆位）
      for (let i = 0; i < N; i++) {
        const pl = plaques[i];
        if (!pl.want) continue;
        pl.el.style.opacity = pl.cull ? "0.12" : "1";
      }

      declutter();
      clouds.follow(pos);
      if (cloudsDirty) {
        clouds.sample(pos, baseColor);
        cloudsDirty = false;
      }
      clouds.update(t);
      composer.render();
      labelRenderer.render(world, camera);
    };

    pushGeometry();
    projectAll();
    refreshAppearance();
    raf = requestAnimationFrame(tick);

    // 「重新布局」：暖机重跑一轮
    const onRelayout = () => restLayout();
    window.addEventListener("graph:relayout", onRelayout);
    const onAppearance = () => refreshAppearance();
    window.addEventListener("graph:appearance", onAppearance);

    return () => {
      // 先存构图与机位：重建时（改依赖、换过滤器…）紧接着就要接着它走
      layoutCache = {
        ids: scene.nodes.map((n) => n.id),
        radius: R,
        pos: new Float32Array(pos),
        camera: [camera.position.x, camera.position.y, camera.position.z],
        target: [controls.target.x, controls.target.y, controls.target.z],
      };
      window.removeEventListener("graph:relayout", onRelayout);
      window.removeEventListener("graph:appearance", onAppearance);
      cancelAnimationFrame(raf);
      ro.disconnect();
      el.removeEventListener("pointermove", onPointerMove);
      el.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("pointerup", onPointerUp);
      el.removeEventListener("click", onClick);
      el.removeEventListener("pointerleave", onLeave);
      controls.dispose();
      nodeGeo.dispose();
      nodeMat.dispose();
      edgeGeo.dispose();
      edgeMat.dispose();
      ring.geometry.dispose();
      ringMat.dispose();
      for (const pl of plaques) pl.obj.removeFromParent();
      stars.dispose();
      clouds.dispose();
      bloom.dispose();
      composer.dispose();
      renderer.dispose();
      el.remove();
      labelHost.remove();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    props.graph,
    props.tagGroups,
    props.ownerOrder,
    props.ownerNames,
    props.tagOrder,
    props.looseness,
  ]);

  // ── 「重新布局」信号 ──
  useEffect(() => {
    if (props.relayoutSignal > 0) {
      window.dispatchEvent(new CustomEvent("graph:relayout"));
    }
  }, [props.relayoutSignal]);

  // ── 外观态变化：只在力场之上重算着色/淡出/铭牌资格，不碰布局 ──
  useEffect(() => {
    window.dispatchEvent(new CustomEvent("graph:appearance"));
  }, [
    props.hoverId,
    props.selectedId,
    props.searchMatchIds,
    props.colorMode,
    props.labelDensity,
    props.edgeWidth,
  ]);

  // 离开图谱页就把缓存丢掉：下次进来仍是定稿机位，只有页内改数据才接着走。
  // 声明在主 effect 之后，卸载时清理按声明顺序跑——先存后清。
  useEffect(() => () => {
    layoutCache = null;
  }, []);

  return (
    <div ref={containerRef} className="graph-canvas h-full w-full" data-testid="graph-canvas" />
  );
}

/** 场景的平均节点半径（最小间距换算用） */
function nodeRadiusOf(scene: Scene): number {
  if (scene.nodes.length === 0) return 2.6;
  let sum = 0;
  for (const n of scene.nodes) sum += n.radius;
  return sum / scene.nodes.length;
}

function hubRole(node: SceneNode): string {
  return node.hubKind === "owner" ? "成员" : node.hubKind === "tag" ? "标签" : "状态";
}
