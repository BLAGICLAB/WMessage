// 力导向物理模拟（任务图谱设计 §3.2）：对齐 Obsidian / d3-force 等效模型——
// 短程斥力 + 连线弹簧 + 向心弱引力 + 碰撞分离，速度 Verlet 积分 + alpha 冷却。
// 纯函数模块（无 DOM/React 依赖），网格分桶让斥力从 O(n²) 降到 O(n·k)。
// 斥力/弹簧常数集中在此处常量，调参不改逻辑。

/** 节点半径：任务 = 3 + √度数×2，hub = 7 + √度数（设计 §3.2） */
export function taskRadius(degree: number): number {
  return 3 + Math.sqrt(degree) * 2;
}

export interface PhysNode {
  id: string;
  x: number;
  y: number;
  vx: number;
  vy: number;
  r: number;
  /** 拖拽固定坐标（非 null 时位置锁死、速度清零） */
  fx: number | null;
  fy: number | null;
}

/** 弹簧（下标引用，由调用方用 id→index 映射构建） */
export interface PhysLink {
  source: number;
  target: number;
  distance: number;
  strength: number;
}

export interface PhysConfig {
  /** 短程斥力强度（cutoff 内按 1/d² 衰减） */
  repulsion: number;
  /** 斥力生效半径（世界坐标） */
  repulsionCutoff: number;
  /** 向心引力系数 */
  gravity: number;
  /** 速度每帧衰减（0.6 = d3 velocityDecay 语义） */
  velocityDecay: number;
  /** alpha 每帧衰减（冷却速率） */
  alphaDecay: number;
  /** alpha 低于此值停止物理循环 */
  alphaMin: number;
  /** 单帧最大位移（防爆） */
  maxStep: number;
  /** 碰撞分离的额外间距 */
  collidePadding: number;
  /** 网格单帧重定位迭代次数 */
  collideIterations: number;
}

export const DEFAULT_CONFIG: PhysConfig = {
  repulsion: 1800,
  repulsionCutoff: 170,
  gravity: 0.03,
  velocityDecay: 0.6,
  alphaDecay: 0.02,
  alphaMin: 0.02,
  maxStep: 24,
  collidePadding: 4,
  collideIterations: 2,
};

/** 弹簧参数（设计 §3.2）：任务依赖紧、成员→hub 松（团簇松散可分） */
export const LINK_DEP = { distance: 90, strength: 0.06 };
export const LINK_MEMBER = { distance: 130, strength: 0.02 };

/** 交互后暖启动的 alpha（过滤切换/拖拽） */
export const REHEAT_ALPHA = 0.5;

/** 初始位置：按序螺旋铺点（确定性的均匀散布，比随机更可测） */
export function initPositions(
  specs: Array<{ id: string; r: number }>,
  width: number,
  height: number
): PhysNode[] {
  const cx = width / 2;
  const cy = height / 2;
  const golden = Math.PI * (3 - Math.sqrt(5));
  return specs.map((s, i) => {
    const radius = 12 * Math.sqrt(i + 1);
    return {
      id: s.id,
      x: cx + Math.cos(i * golden) * radius,
      y: cy + Math.sin(i * golden) * radius,
      vx: 0,
      vy: 0,
      r: s.r,
      fx: null,
      fy: null,
    };
  });
}

/** 按 id 保留既有位置重建（过滤变更时不闪跳） */
export function rebindPositions(
  prev: PhysNode[],
  specs: Array<{ id: string; r: number }>,
  width: number,
  height: number
): PhysNode[] {
  const fresh = initPositions(specs, width, height);
  const byId = new Map(prev.map((n) => [n.id, n]));
  return fresh.map((n) => {
    const p = byId.get(n.id);
    return p ? { ...p, r: n.r } : n;
  });
}

interface Grid {
  cell: number;
  cols: number;
  originX: number;
  originY: number;
  buckets: Map<number, number[]>;
}

function buildGrid(nodes: PhysNode[], cutoff: number): Grid {
  let minX = Infinity;
  let minY = Infinity;
  for (const n of nodes) {
    if (n.x < minX) minX = n.x;
    if (n.y < minY) minY = n.y;
  }
  const cols = Math.ceil(Math.sqrt(Math.max(nodes.length, 1))) + 1;
  const buckets = new Map<number, number[]>();
  for (let i = 0; i < nodes.length; i++) {
    const gx = Math.floor((nodes[i].x - minX) / cutoff);
    const gy = Math.floor((nodes[i].y - minY) / cutoff);
    const key = gx * cols + gy;
    const list = buckets.get(key);
    if (list) list.push(i);
    else buckets.set(key, [i]);
  }
  return { cell: cutoff, cols, originX: minX, originY: minY, buckets };
}

/**
 * 单步积分：斥力 + 弹簧 + 向心 → 速度衰减 → 位移；再跑碰撞分离。
 * 返回衰减后的 alpha（调用方据此判断是否停循环）。节点数为 0/1 时只衰 alpha。
 */
export function tick(
  nodes: PhysNode[],
  links: PhysLink[],
  cfg: PhysConfig,
  alpha: number,
  centerX: number,
  centerY: number
): number {
  const nextAlpha = alpha * (1 - cfg.alphaDecay);
  if (nodes.length <= 1) return nextAlpha;
  const grid = buildGrid(nodes, cfg.repulsionCutoff);

  // 短程斥力（cutoff 内 1/d²，按半径加权）
  for (let i = 0; i < nodes.length; i++) {
    const a = nodes[i];
    const gx = Math.floor((a.x - grid.originX) / grid.cell);
    const gy = Math.floor((a.y - grid.originY) / grid.cell);
    for (let ox = -1; ox <= 1; ox++) {
      for (let oy = -1; oy <= 1; oy++) {
        const bucket = grid.buckets.get((gx + ox) * grid.cols + (gy + oy));
        if (!bucket) continue;
        for (const j of bucket) {
          if (j <= i) continue;
          const b = nodes[j];
          let dx = a.x - b.x;
          let dy = a.y - b.y;
          let d2 = dx * dx + dy * dy;
          if (d2 === 0) {
            // 完全重合：确定性拆开（避免 NaN 与抖动）
            dx = 0.5;
            dy = 0.25;
            d2 = dx * dx + dy * dy;
          }
          if (d2 > cfg.repulsionCutoff * cfg.repulsionCutoff) continue;
          const d = Math.sqrt(d2);
          const f =
            (cfg.repulsion * alpha * ((a.r + b.r) * 2)) / d2;
          const fx = (dx / d) * f;
          const fy = (dy / d) * f;
          a.vx += Math.max(-cfg.maxStep, Math.min(cfg.maxStep, fx));
          a.vy += Math.max(-cfg.maxStep, Math.min(cfg.maxStep, fy));
          b.vx -= Math.max(-cfg.maxStep, Math.min(cfg.maxStep, fx));
          b.vy -= Math.max(-cfg.maxStep, Math.min(cfg.maxStep, fy));
        }
      }
    }
  }

  // 连线弹簧
  for (const l of links) {
    const a = nodes[l.source];
    const b = nodes[l.target];
    if (!a || !b) continue;
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const d = Math.sqrt(dx * dx + dy * dy) || 0.01;
    const k = ((d - l.distance) / d) * l.strength * alpha;
    a.vx += dx * k;
    a.vy += dy * k;
    b.vx -= dx * k;
    b.vy -= dy * k;
  }

  // 向心引力 + 积分（速度衰减；固定点锁位）
  for (const n of nodes) {
    n.vx += (centerX - n.x) * cfg.gravity * alpha;
    n.vy += (centerY - n.y) * cfg.gravity * alpha;
    if (n.fx !== null && n.fy !== null) {
      n.x = n.fx;
      n.y = n.fy;
      n.vx = 0;
      n.vy = 0;
      continue;
    }
    n.vx *= 1 - cfg.velocityDecay;
    n.vy *= 1 - cfg.velocityDecay;
    n.x += Math.max(-cfg.maxStep, Math.min(cfg.maxStep, n.vx));
    n.y += Math.max(-cfg.maxStep, Math.min(cfg.maxStep, n.vy));
  }

  // 碰撞分离（位置修正式，半径和 + padding）
  for (let iter = 0; iter < cfg.collideIterations; iter++) {
    const g = buildGrid(nodes, cfg.repulsionCutoff);
    for (let i = 0; i < nodes.length; i++) {
      const a = nodes[i];
      const gx = Math.floor((a.x - g.originX) / g.cell);
      const gy = Math.floor((a.y - g.originY) / g.cell);
      for (let ox = -1; ox <= 1; ox++) {
        for (let oy = -1; oy <= 1; oy++) {
          const bucket = g.buckets.get((gx + ox) * g.cols + (gy + oy));
          if (!bucket) continue;
          for (const j of bucket) {
            if (j <= i) continue;
            const b = nodes[j];
            const min = a.r + b.r + cfg.collidePadding;
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let d2 = dx * dx + dy * dy;
            if (d2 >= min * min) continue;
            if (d2 === 0) {
              dx = 0.5;
              dy = 0.25;
              d2 = dx * dx + dy * dy;
            }
            const d = Math.sqrt(d2);
            const push = ((min - d) / d) * 0.5;
            const px = dx * push;
            const py = dy * push;
            if (a.fx === null) {
              a.x -= px * 0.5;
              a.y -= py * 0.5;
            }
            if (b.fx === null) {
              b.x += px * 0.5;
              b.y += py * 0.5;
            }
          }
        }
      }
    }
  }
  return nextAlpha;
}
