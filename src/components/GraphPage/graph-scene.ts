// 3D 图谱场景（星云方案）：渲染无关的场景描述 + 锚点岗位。
// 纯逻辑，无 React / DOM / three 依赖——场景结构是布局与渲染的共同上游，
// 关系写错会在图上表现为「同标签的任务被拉散」，很难从画面反推，改这里必有单测。
//
// 结构：任务节点 + 三类结构锚点（成员核 / 标签核 / 状态核）。锚点是「路牌」——
// 三维里没有平面分扇区可依附，全靠锚点把同族任务聚成可辨认的星团；
// 没有锚点，三维力场会把所有任务揉成一团毛球。

import type { ColumnId } from "../../types";
import type { BuiltGraph, GraphNode } from "./graph-build";
import { ownerColorKey, tagColorKey, STALL_DAYS } from "./graph-palette";
import type { GraphLooseness } from "../../lib/graphPrefs";

type HubKind = "owner" | "tag" | "status";

/** 结构锚点 id 前缀。对外只暴露 isHub()，前缀本身不外泄——外部没有拼 id 的理由 */
const HUB_PREFIX = "hub:";

export interface SceneNode {
  id: string;
  kind: "task" | "hub";
  hubKind?: HubKind;
  label: string;
  radius: number;
  /** 任务节点：真实连接度（dep + parent）。锚点 = 挂在它上面的任务数 */
  degree: number;
  /** 着色语义键（建图时固化，切着色模式不重建布局） */
  ownerKey: string;
  tagIdx: number | null;
  statusKey: ColumnId;
  /** 进行中且已历 STALL_DAYS 天以上 */
  stalled: boolean;
  /** 锚点的岗位坐标（世界单位）；任务节点为 null */
  hint: readonly [number, number, number] | null;
}

interface SceneEdge {
  a: number;
  b: number;
  weight: number;
  kind: "dep" | "parent" | "hub";
  /** 静息长度（世界单位）：弹簧的平衡距离，不是让两端贴死 */
  rest: number;
}

/**
 * 静息长度占布局外径的比例。比例而非绝对值——收敛后全场归一到 R，
 * 按比例给才能让千级和万级任务长出同一片星云。
 *
 * 这一项是锚点体系能不能撑开空间的全部原因：成员核/标签核/状态核铺在外壳上，
 * 而每张卡同时被三条锚定边拉住。若弹簧只有强度没有静息长度，合力方向就是
 * 「三核加权形心」，而形心几乎落在原点——所有任务被拽成一颗过曝的白球，
 * 节点和铭牌互相糊死。给了静息长度，卡才会被顶在三核外侧，星云才填得满。
 * 数值取自视觉小样的力场实测比例。
 */
const REST_RATIO = {
  /** 任务间关系贴得近，是图里的「骨」 */
  task: 0.093,
  /** 标签核权重最高，静息最近——成图按标签成团 */
  tag: 0.372,
  owner: 0.512,
  status: 0.574,
} as const;

export interface Scene {
  nodes: SceneNode[];
  edges: SceneEdge[];
  /** 节点 id → 下标（拾取/悬停/搜索查表） */
  indexOf: Map<string, number>;
  /** 布局外径（力场收敛后归一化到这个半径） */
  radius: number;
}

/** 组内任务不足此数不建标签锚点（单卡标签没有聚簇意义） */
const HUB_MIN_MEMBERS = 2;

/** 布局外径系数（面密度恒定：R = k·√N，千级与万级任务的视口密度一致） */
const LOOSENESS_R: Record<GraphLooseness, number> = {
  tight: 30,
  standard: 45,
  loose: 70,
};

/**
 * 星点屏幕直径的下限（CSS 像素）。
 *
 * 星点的屏幕直径 ≈ 798 × (半径 / 布局外径)：布局外径 R = k√N 随任务数涨，
 * 而半径是按连接度给的定值，于是两者一除，星点在大图里越缩越小——
 * 万级任务时直径不到 0.5px，落在像素栅格以下（驱动按 1px 画，形状与
 * 亮度都失真），整片星云退成一层看不清的灰点。
 *
 * 下限按外径给（而不是给一个绝对世界值）：布局放大多少，星点的地板
 * 就跟着放大多少，屏幕上的观感在任何规模下都一致。
 */
export const NODE_PX_MIN = 1.6;

/**
 * 世界半径 → 屏幕直径的换算系数：点径 px ≈ 798 × (半径 / 布局外径)。
 * 798 = 渲染层的点径系数 2300 ÷ 机位距离比 2.882（相机按外径等比摆）。
 * 渲染层改了机位或点径系数，这里要跟着改——graph-scene.test.ts 有对拍。
 */
export const SCREEN_PX_PER_RATIO = 798;

/**
 * 任务半径只跟连接数走（√压缩：度 2≈2.6、9≈5.6、25≈8.12、100≈14）。
 * 「按耗时定大小」已退役——拖得久不等于牵涉多，两套语义混在一张图上互相拆台。
 *
 * 半径另有一个下限：星点跟着布局外径一起缩放，小到看不见就没意义了。
 */
export function nodeRadius(degree: number, R: number): number {
  // 扣掉两条基础连线（每张卡平均都有）免得低度数节点缩成看不见的点
  const pool = Math.max(degree - 2, 0);
  const raw = 2.6 + Math.sqrt(pool) * 1.15;
  return Math.max(raw, minWorldRadius(R));
}

/** 屏幕下限对应的世界半径：布局外径越大，同样的屏幕尺寸要更多世界单位 */
function minWorldRadius(R: number): number {
  return (R * NODE_PX_MIN) / SCREEN_PX_PER_RATIO;
}

/** 锚点半径下界：够大到读得出「这里是个核」，又不能盖过它聚起来的任务 */
const HUB_RADIUS_FLOOR = 5.0;

/** 岗位坐标的参考半径（星云方案的构图基准，坐标按 R/此值缩放） */
const HINT_REF_R = 258;

export interface SceneOptions {
  /** 成员注入序（chips 全序，单一事实源） */
  ownerOrder: Map<string, number>;
  /** 标签注入序（按使用数降序） */
  tagOrder: Map<string, number>;
  looseness: GraphLooseness;
  now: number;
  /** 标签近义组（Rust 嵌入 + 并查集）；缺省时每个标签自成一组 */
  tagGroups?: Map<string, string>;
  /**
   * 成员语义键（ownerColorKey 的输出）→ 显示名。锚点铭牌直接印在画布上，
   * 印 `owner:0` 这种内部键等于把数据结构漏给用户看，必须换成真名。
   */
  ownerNames?: Map<string, string>;
}

/** 状态锚点的显示名（铭牌面向人，列 id 不该出现） */
const STATUS_HUB_LABEL: Record<ColumnId, string> = {
  todo: "待办",
  doing: "进行中",
  done: "已完成",
};

/**
 * 锚点岗位（坐标直接决定成图可读性，不能随便挪）。
 *
 * 三套坐标都取自视觉小样的实测值，不是随手摆的形状：
 *  - 成员核：一条向后弯的浅弧。正对镜头时一字排开的铭牌会叠成一行，
 *    弧线（y 起伏 + z 随 |x| 递增）把它们拉开纵深。
 *  - 标签核：黄金角 Fibonacci 球面，y 只铺 1.75 个身位——铺满 [-1,1] 会在正上
 *    正下各挤出一枚半径趋零的针尖。标签之间没有亲缘，均匀铺开才不打架。
 *  - 状态核：竖直一列且间距拉开。状态是有序维度，列排比散排好读。
 *
 * 小样的 6 成员 / 10 标签代入下面公式逐字复现；换成别的个数时 x 按下标均分
 * （弧形与 z 的关系不变），铭牌挤了交给渲染层避让，不在这里造随机扰动——
 * 场景必须可复现，同一份数据每次打开都得长成同一个样。
 */
function hubHints(
  ownerKeys: string[],
  tagGroups: string[],
  statuses: ColumnId[],
  R: number,
): Map<string, readonly [number, number, number]> {
  const k = R / HINT_REF_R;
  const out = new Map<string, readonly [number, number, number]>();
  ownerKeys.forEach((key, i) => {
    const t = ownerKeys.length === 1 ? 0.5 : i / (ownerKeys.length - 1);
    const x = (t - 0.5) * 304;
    out.set(`${HUB_PREFIX}owner:${key}`, [
      x * k,
      (-74 + Math.sin(i * 1.05) * 26) * k,
      (30 + Math.abs(x) * 0.42) * k,
    ]);
  });
  const golden = Math.PI * (3 - Math.sqrt(5));
  tagGroups.forEach((g, i) => {
    const y = 1 - ((i + 0.5) / tagGroups.length) * 1.75;
    const r = Math.sqrt(Math.max(0, 1 - y * y));
    const th = golden * i;
    out.set(`${HUB_PREFIX}tag:${g}`, [
      Math.cos(th) * r * 176 * k,
      y * 126 * k,
      Math.sin(th) * r * 176 * k,
    ]);
  });
  statuses.forEach((s, i) => {
    out.set(`${HUB_PREFIX}status:${s}`, [
      -26 * k,
      (-54 + i * 54) * k,
      34 * k,
    ]);
  });
  return out;
}

function collect<K>(into: Map<K, GraphNode[]>, key: K, n: GraphNode): void {
  const list = into.get(key);
  if (list) list.push(n);
  else into.set(key, [n]);
}

const DAY_MS = 86_400_000;

/** 进行中且已历 STALL_DAYS 天以上（无创建时间的老数据不算：它不是钉子户，是没记时间） */
export function isStalled(
  n: Pick<GraphNode, "status" | "createdAt">,
  now: number,
): boolean {
  if (!n.createdAt || n.status !== "doing") return false;
  return (now - n.createdAt) / DAY_MS >= STALL_DAYS;
}

/**
 * 建图 → 三维场景（含锚点与锚定边）。
 *
 * 锚定边权重：标签核 3、成员核 2、状态核 1.5。标签最强是因为「同标签 = 同类
 * 工作」是这张图最强的先验；状态最弱是因为状态随时会变，压太紧会把图锁死在
 * 当前列分布上，任务一改状态整块就重排——图谱不该比数据还敏感。
 */
export function buildScene(built: BuiltGraph, opts: SceneOptions): Scene {
  const { ownerOrder, tagOrder, tagGroups, ownerNames, now, looseness } = opts;
  const tasks = built.nodes.filter((n) => n.kind === "task");
  const R = LOOSENESS_R[looseness] * Math.sqrt(Math.max(tasks.length, 1));

  // ── 锚点候选：只给「有成员」的维度建核，空核漂在宇宙里只是噪声 ──
  const ownerMembers = new Map<string, GraphNode[]>();
  const statusMembers = new Map<ColumnId, GraphNode[]>();
  const tagMembers = new Map<string, GraphNode[]>();
  for (const n of tasks) {
    collect(ownerMembers, ownerColorKey(n, ownerOrder), n);
    collect(statusMembers, n.status ?? "todo", n);
    // 一个任务可入多个同义组（每组只登记一次，避免同组重复挂边）
    const seen = new Set<string>();
    for (const tag of n.tags ?? []) {
      if (!tag) continue;
      const g = tagGroups?.get(tag) ?? tag;
      if (seen.has(g)) continue;
      seen.add(g);
      collect(tagMembers, g, n);
    }
  }

  const tagGroupNames = new Set(
    [...tagMembers.entries()]
      .filter(([, m]) => m.length >= HUB_MIN_MEMBERS)
      .map(([g]) => g),
  );
  const ownerKeys = [...ownerMembers.keys()].sort();
  const statuses = (["todo", "doing", "done"] as const).filter((s) =>
    statusMembers.has(s),
  );
  const hints = hubHints(ownerKeys, [...tagGroupNames].sort(), statuses, R);

  const nodes: SceneNode[] = [];
  const indexOf = new Map<string, number>();
  const push = (n: SceneNode) => {
    indexOf.set(n.id, nodes.length);
    nodes.push(n);
  };

  // 任务节点按 tasks 原序入列：后面的锚定边直接用这个下标
  tasks.forEach((n) => {
    const status = n.status ?? "todo";
    push({
      id: n.id,
      kind: "task",
      label: n.label,
      radius: nodeRadius(n.degree, R),
      degree: n.degree,
      ownerKey: ownerColorKey(n, ownerOrder),
      tagIdx: tagColorKey(n, tagOrder),
      statusKey: status,
      stalled: isStalled(n, now),
      hint: null,
    });
  });

  const hubIdOf = (kind: HubKind, key: string) => `${HUB_PREFIX}${kind}:${key}`;
  const hub = (
    kind: HubKind,
    key: string,
    members: number,
    label: string,
  ) => {
    push({
      id: hubIdOf(kind, key),
      kind: "hub",
      hubKind: kind,
      label,
      radius: Math.max(nodeRadius(members, R), HUB_RADIUS_FLOOR, minWorldRadius(R)),
      degree: members,
      ownerKey: "self",
      tagIdx: null,
      statusKey: "todo",
      stalled: false,
      hint: hints.get(hubIdOf(kind, key)) ?? null,
    });
  };
  ownerKeys.forEach((key) =>
    hub("owner", key, ownerMembers.get(key)!.length, ownerNames?.get(key) ?? key),
  );
  statuses.forEach((s) =>
    hub("status", s, statusMembers.get(s)!.length, STATUS_HUB_LABEL[s]),
  );
  [...tagGroupNames].forEach((g) => hub("tag", g, tagMembers.get(g)!.length, g));

  // ── 边：任务间关系 + 锚定边 ──
  const edges: SceneEdge[] = [];
  const seenEdge = new Set<string>();
  const link = (
    a: number,
    b: number | undefined,
    weight: number,
    kind: SceneEdge["kind"],
    rest: number,
  ) => {
    if (b === undefined || a === b) return;
    const key = a < b ? `${a}-${b}` : `${b}-${a}`;
    if (seenEdge.has(key)) return;
    seenEdge.add(key);
    edges.push({ a, b, weight, kind, rest: R * rest });
  };
  for (const l of built.links) {
    link(indexOf.get(l.source)!, indexOf.get(l.target)!, 1, l.kind, REST_RATIO.task);
  }
  tasks.forEach((n, i) => {
    link(i, indexOf.get(hubIdOf("owner", ownerColorKey(n, ownerOrder))), 2, "hub", REST_RATIO.owner);
    link(i, indexOf.get(hubIdOf("status", n.status ?? "todo")), 1.5, "hub", REST_RATIO.status);
    const seen = new Set<string>();
    for (const tag of n.tags ?? []) {
      if (!tag) continue;
      const g = tagGroups?.get(tag) ?? tag;
      if (seen.has(g) || !tagGroupNames.has(g)) continue;
      seen.add(g);
      link(i, indexOf.get(hubIdOf("tag", g)), 3, "hub", REST_RATIO.tag);
    }
  });

  return { nodes, edges, indexOf, radius: R };
}

/** 是不是结构锚点（交互层据此挡掉业务点选——锚点不是任务） */
export function isHub(id: string): boolean {
  return id.startsWith(HUB_PREFIX);
}

/**
 * 「标准」标签密度的常驻门槛：取任务度数分布的 P93 分位。
 *
 * 门槛写死成固定度数会让密度档随图的疏密漂移：设计稿那张 400 条边的图满屏标签，
 * 稀疏到 40 条边的真实任务库一个都不出——档位形同虚设。按分位取，稀疏图照样
 * 露出最重的那几颗，密图也不会糊成字墙。
 */
export function labelDegreeFloor(nodes: SceneNode[]): number {
  const degs = nodes
    .filter((n) => n.kind === "task")
    .map((n) => n.degree)
    .sort((a, b) => a - b);
  if (degs.length === 0) return 1;
  return degs[Math.min(degs.length - 1, Math.floor(degs.length * 0.93))];
}
