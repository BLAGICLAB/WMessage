// 三维力场：斥力（空间网格分桶）+ 线性 Hooke 弹簧 + 中心引力 + 锚点回拉。
//
// 为什么自己写而不复用 FA2：FA2 是二维的，三维图谱需要第三个维度的斥力与
// 分桶，本项目现有的 graphology-layout-forceatlas2 给不了；塞进 Web Worker 又要
// 另写一套图/worker 协议（首页速览卡还在用同步版 FA2，不该为此动它）。
//
// 为什么用空间网格而不是 O(N²)：现有图谱规模上限是万级任务，O(N²) 一步是
// 5×10⁷ 次乘加，单帧吃不消。网格把斥力限定在截断半径内，步长与规模近似解耦。
//
// 为什么用定温位移而不是速度积分：速度积分的稳态速度是力的 1/(1−阻尼) 倍，
// 力场里永远存在不衰减的斥力，稳态速度就被钳死在上限、动能降不到收敛判据
// 以下（表现为「永远在抖，停不下来」）。定温位移每步位移有硬上限且温度单调
// 下降，收敛是结构保证的，不依赖参数调得好不好。

import type { Scene } from "./graph-scene";

export interface ForceOptions {
  /** 斥力截断半径（世界单位）：超过即不计入，同时是网格的桶边长 */
  repulseCutoff: number;
  /** 斥力强度系数（实际力 = repulse / (d² + softGap²)） */
  repulse: number;
  /** 软核半径：近重合时斥力封顶在 repulse/softGap²，不发散也不抖 */
  softGap: number;
  /** 弹簧强度（线性 Hooke：f = spring·w·d，不含 1/d） */
  spring: number;
  /** 中心引力：把散出去的节点收回来，防止图无限膨胀 */
  gravity: number;
  /** 锚点回拉强度：锚点靠它守住岗位（写成位置修正，不走力） */
  hubPull: number;
  /** 初始步长（世界单位/步）：前几十步放开跑，靠降温自己收住 */
  startTemp: number;
  /** 每步降温系数 */
  cooling: number;
  /** 步长下限：降到此值即视为静止，不再继续跑 */
  minTemp: number;
}

const DEFAULT_FORCE: ForceOptions = {
  repulseCutoff: 96,
  repulse: 1900,
  softGap: 18,
  spring: 0.05,
  gravity: 0.0022,
  hubPull: 0.24,
  startTemp: 14,
  cooling: 0.988,
  minTemp: 0.012,
};

/** 收敛判据：全场平均位移平方和低于此值即认为静止 */
export const KINETIC_EPSILON = 4e-4;

/** 单轴最大桶数：防住「一个跑飞的节点把网格撑到几亿桶」 */
const MAX_GRID_DIM = 512;

/** mulberry32：小而稳的种子发生器。固定种子 = 每次打开是同一片星云，
 *  构图可回归；用 Math.random 会让「昨天好看的那张图」再也复现不了。 */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const DEFAULT_SEED = 0x5eed1a;

function clampInt(v: number, lo: number, hi: number): number {
  if (!(v > lo)) return lo; // 同时挡住 NaN
  return v > hi ? hi : v;
}

function clampDim(v: number): number {
  if (!Number.isFinite(v) || v < 1) return 1;
  return Math.min(MAX_GRID_DIM, Math.floor(v));
}

/** 均匀网格（计数排序 CSR）：按截断半径分桶，斥力只查 27 邻桶 */
class Grid {
  nx = 1;
  private ny = 1;
  private nz = 1;
  private minX = 0;
  private minY = 0;
  private minZ = 0;
  private cell = 1;
  private dims = 0;
  private offsets = new Int32Array(2);
  private items = new Int32Array(0);

  build(pos: Float32Array, n: number, cell: number): void {
    this.cell = cell;
    let minX = Infinity, minY = Infinity, minZ = Infinity;
    let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
    for (let i = 0; i < n; i++) {
      const x = pos[i * 3], y = pos[i * 3 + 1], z = pos[i * 3 + 2];
      if (x < minX) minX = x;
      if (x > maxX) maxX = x;
      if (y < minY) minY = y;
      if (y > maxY) maxY = y;
      if (z < minZ) minZ = z;
      if (z > maxZ) maxZ = z;
    }
    if (!Number.isFinite(minX)) minX = maxX = minY = maxY = minZ = maxZ = 0;
    this.minX = minX;
    this.minY = minY;
    this.minZ = minZ;
    this.nx = clampDim(Math.ceil((maxX - minX) / cell) + 1);
    this.ny = clampDim(Math.ceil((maxY - minY) / cell) + 1);
    this.nz = clampDim(Math.ceil((maxZ - minZ) / cell) + 1);
    this.dims = this.nx * this.ny * this.nz;
    if (this.offsets.length < this.dims + 1 || this.items.length < n) {
      this.offsets = new Int32Array(this.dims + 1);
      this.items = new Int32Array(Math.max(n, 1));
    }
    const counts = new Int32Array(this.dims + 1);
    for (let i = 0; i < n; i++) counts[this.flat(this.cellOf(pos, i))]++;
    let acc = 0;
    for (let c = 0; c < this.dims; c++) {
      this.offsets[c] = acc;
      acc += counts[c];
    }
    this.offsets[this.dims] = acc;
    const cursor = this.offsets.slice(0, this.dims);
    for (let i = 0; i < n; i++) this.items[cursor[this.flat(this.cellOf(pos, i))]++] = i;
  }

  /** 节点 i 所在桶的三维坐标（out 可复用，避免每次调用都分配） */
  cellOf(pos: Float32Array, i: number, out = SCRATCH): Int32Array {
    out[0] = clampInt(Math.floor((pos[i * 3] - this.minX) / this.cell), 0, this.nx - 1);
    out[1] = clampInt(Math.floor((pos[i * 3 + 1] - this.minY) / this.cell), 0, this.ny - 1);
    out[2] = clampInt(Math.floor((pos[i * 3 + 2] - this.minZ) / this.cell), 0, this.nz - 1);
    return out;
  }

  private flat(c: Int32Array): number {
    return (c[0] * this.ny + c[1]) * this.nz + c[2];
  }

  /** 邻桶平面下标（越界返回 -1） */
  neighborFlat(cx: number, cy: number, cz: number): number {
    if (cx < 0 || cx >= this.nx || cy < 0 || cy >= this.ny || cz < 0 || cz >= this.nz) {
      return -1;
    }
    return (cx * this.ny + cy) * this.nz + cz;
  }

  range(flat: number): number {
    return this.offsets[flat + 1] - this.offsets[flat];
  }

  at(flat: number, k: number): number {
    return this.items[this.offsets[flat] + k];
  }
}

const SCRATCH = new Int32Array(3);

/** 力的可运行体：持有位置/速度/受力缓冲与网格。跑在渲染循环之外，
 *  由调用方按帧预算调用 step()，或一次 settle() 到底再交给渲染层。 */
export class Force3D {
  readonly scene: Scene;
  readonly options: ForceOptions;
  readonly pos: Float32Array;
  readonly vel: Float32Array;
  private readonly force: Float32Array;
  /** 锚点岗位的私有副本（任务节点全 0）。Scene 里那份是只读共享的，
   *  归一化时要跟着坐标一起缩放，不能就地改。 */
  private readonly hints: Float32Array;
  private readonly grid = new Grid();
  private readonly coord = new Int32Array(3);
  private tempNow: number;
  private repaired = 0;

  constructor(scene: Scene, options: Partial<ForceOptions> = {}) {
    this.scene = scene;
    this.options = { ...DEFAULT_FORCE, ...options };
    const n = scene.nodes.length;
    this.pos = new Float32Array(n * 3);
    this.vel = new Float32Array(n * 3);
    this.force = new Float32Array(n * 3);
    this.hints = new Float32Array(n * 3);
    for (let i = 0; i < n; i++) {
      const hint = scene.nodes[i].hint;
      if (!hint) continue;
      this.hints[i * 3] = hint[0];
      this.hints[i * 3 + 1] = hint[1];
      this.hints[i * 3 + 2] = hint[2];
    }
    this.tempNow = this.options.startTemp;
    this.seed();
  }

  /** 把温度退回起点（「重新布局」重跑一轮用） */
  reheat(): void {
    this.tempNow = this.options.startTemp;
    this.vel.fill(0);
  }

  /** 初值：锚点从岗位出发（带随机偏移破对称），任务在球内 Fibonacci 均匀撒开 */
  private seed(): void {
    const rand = mulberry32(DEFAULT_SEED);
    const n = this.scene.nodes.length;
    const R = this.scene.radius;
    const golden = Math.PI * (3 - Math.sqrt(5));
    for (let i = 0; i < n; i++) {
      const p = i * 3;
      if (this.hints[p] !== 0 || this.hints[p + 1] !== 0 || this.hints[p + 2] !== 0) {
        this.pos[p] = this.hints[p] + (rand() - 0.5) * 6;
        this.pos[p + 1] = this.hints[p + 1] + (rand() - 0.5) * 6;
        this.pos[p + 2] = this.hints[p + 2] + (rand() - 0.5) * 6;
        continue;
      }
      const y = 1 - ((i + 0.5) / Math.max(n, 1)) * 2;
      const r = Math.sqrt(Math.max(0, 1 - y * y));
      const th = golden * i;
      const rad = R * (0.35 + 0.6 * Math.cbrt(rand()));
      this.pos[p] = Math.cos(th) * r * rad;
      this.pos[p + 1] = y * rad * 0.7;
      this.pos[p + 2] = Math.sin(th) * r * rad;
    }
  }

  /** 本次运行里被坐标消毒救回来的节点数（诊断用，非 0 即力场发散过） */
  get repairCount(): number {
    return this.repaired;
  }

  /** 全场平均位移平方和（定温布局的动能：位移恒 ≤ temp，所以它跟着温度一起归零） */
  kinetic(): number {
    if (this.vel.length === 0) return 0;
    let sum = 0;
    for (let i = 0; i < this.vel.length; i += 3) {
      sum += this.vel[i] ** 2 + this.vel[i + 1] ** 2 + this.vel[i + 2] ** 2;
    }
    return sum / (this.vel.length / 3);
  }

  /** 当前温度（= 单步位移上限） */
  get temp(): number {
    return this.tempNow;
  }

  step(): void {
    const { pos, vel, force, options } = this;
    const n = this.scene.nodes.length;
    if (n === 0) return;
    force.fill(0);
    this.grid.build(pos, n, options.repulseCutoff);

    // ── 斥力（网格 27 邻桶，截断半径内；软核封顶，近重合不发散） ──
    const cut2 = options.repulseCutoff * options.repulseCutoff;
    const soft2 = options.softGap * options.softGap;
    for (let i = 0; i < n; i++) {
      const p = i * 3;
      const xi = pos[p], yi = pos[p + 1], zi = pos[p + 2];
      this.grid.cellOf(pos, i, this.coord);
      const cx = this.coord[0], cy = this.coord[1], cz = this.coord[2];
      for (let dx = -1; dx <= 1; dx++) {
        for (let dy = -1; dy <= 1; dy++) {
          for (let dz = -1; dz <= 1; dz++) {
            const flat = this.grid.neighborFlat(cx + dx, cy + dy, cz + dz);
            if (flat < 0) continue;
            const count = this.grid.range(flat);
            for (let k = 0; k < count; k++) {
              const j = this.grid.at(flat, k);
              if (j === i) continue;
              const q = j * 3;
              const ex = xi - pos[q], ey = yi - pos[q + 1], ez = zi - pos[q + 2];
              const d2 = ex * ex + ey * ey + ez * ez;
              if (d2 > cut2) continue;
              let f: number;
              let ux: number, uy: number, uz: number;
              if (d2 < 1e-6) {
                // 近重合：方向不可靠，沿确定性的微扰方向推（同下标永远同向，
                // 不会两个节点互推成死循环）
                f = options.repulse / soft2;
                ux = ((i % 7) - 3) / 5;
                uy = ((j % 5) - 2) / 5;
                uz = ((i + j) % 3 - 1) / 3;
              } else {
                const d = Math.sqrt(d2);
                f = options.repulse / (d2 + soft2);
                ux = ex / d;
                uy = ey / d;
                uz = ez / d;
              }
              force[p] += ux * f;
              force[p + 1] += uy * f;
              force[p + 2] += uz * f;
            }
          }
        }
      }
    }

    // ── 弹簧（线性 Hooke：f = k·w·(d − 静息长度)，力正比于伸长量） ──
    // 少了静息长度这一项，锚定边的合力就只指向「三核加权形心」，全场会被
    // 拽成一颗密不透光的核；减掉静息长度后卡才停在锚点外侧。
    for (const e of this.scene.edges) {
      const a = e.a * 3;
      const b = e.b * 3;
      const dx = pos[b] - pos[a];
      const dy = pos[b + 1] - pos[a + 1];
      const dz = pos[b + 2] - pos[a + 2];
      const d = Math.sqrt(dx * dx + dy * dy + dz * dz);
      if (d < 1e-6) continue;
      const f = options.spring * e.weight * (d - e.rest);
      const ux = (dx / d) * f;
      const uy = (dy / d) * f;
      const uz = (dz / d) * f;
      force[a] += ux; force[a + 1] += uy; force[a + 2] += uz;
      force[b] -= ux; force[b + 1] -= uy; force[b + 2] -= uz;
    }

    // ── 中心引力 ──
    for (let i = 0; i < n; i++) {
      const p = i * 3;
      force[p] -= options.gravity * pos[p];
      force[p + 1] -= options.gravity * pos[p + 1];
      force[p + 2] -= options.gravity * pos[p + 2];
      const hint = this.scene.nodes[i].hint;
      if (hint) {
        // 锚点回拉写成位置修正而非力：走力的话，收敛末期会在岗位两侧来回荡
        pos[p] += (this.hints[p] - pos[p]) * options.hubPull;
        pos[p + 1] += (this.hints[p + 1] - pos[p + 1]) * options.hubPull;
        pos[p + 2] += (this.hints[p + 2] - pos[p + 2]) * options.hubPull;
      }
      // 定温位移：方向取合力，步长封顶在当前温度——位移永远不会超过 temp，
      // 温度单调下降 ⇒ 收敛是结构保证的
      const mag = Math.hypot(force[p], force[p + 1], force[p + 2]);
      if (mag > 1e-9) {
        const s = Math.min(1, this.temp / mag);
        const dx = force[p] * s;
        const dy = force[p + 1] * s;
        const dz = force[p + 2] * s;
        vel[p] = dx;
        vel[p + 1] = dy;
        vel[p + 2] = dz;
        pos[p] += dx;
        pos[p + 1] += dy;
        pos[p + 2] += dz;
      } else {
        vel[p] = vel[p + 1] = vel[p + 2] = 0;
      }
    }
    this.sanitize();
    this.tempNow = Math.max(this.tempNow * options.cooling, options.minTemp);
  }

  /**
   * 坐标消毒：非有限值 / 越界节点重置回确定性环带。
   * 渲染层按这些坐标写顶点缓冲，一个坏节点会让整批图元在 GPU 上被丢弃，
   * 表现为「整帧闪一下就没了」——这类故障只能在数据层堵。
   */
  private sanitize(): void {
    const { pos, vel } = this;
    const bound = this.scene.radius * 6;
    const golden = 2.399963;
    const R = this.scene.radius;
    for (let i = 0; i < this.scene.nodes.length; i++) {
      const p = i * 3;
      if (
        Number.isFinite(pos[p]) &&
        Number.isFinite(pos[p + 1]) &&
        Number.isFinite(pos[p + 2]) &&
        Math.abs(pos[p]) <= bound &&
        Math.abs(pos[p + 1]) <= bound &&
        Math.abs(pos[p + 2]) <= bound
      ) {
        continue;
      }
      const ang = (i + 1) * golden;
      const r = R * (0.3 + ((i % 11) / 11) * 0.5);
      pos[p] = Math.cos(ang) * r;
      pos[p + 1] = Math.sin(ang * 1.3) * r * 0.5;
      pos[p + 2] = Math.sin(ang) * r;
      vel[p] = vel[p + 1] = vel[p + 2] = 0;
      this.repaired++;
    }
  }

  /** 收敛后把全场缩放到目标外径（松散度设置因此对最终画面生效） */
  normalize(targetRadius: number): void {
    let max = 0;
    for (let i = 0; i < this.pos.length; i += 3) {
      const r = Math.hypot(this.pos[i], this.pos[i + 1], this.pos[i + 2]);
      if (r > max) max = r;
    }
    if (!Number.isFinite(max) || max < 1e-6) return;
    const k = targetRadius / max;
    for (let i = 0; i < this.pos.length; i++) this.pos[i] *= k;
    // 岗位跟着一起缩放：坐标换了尺度而岗位没换，下一轮 hubPull 会把锚点
    // 从新尺度拽回旧尺度，整片星云会在拖拽松手后跳一次
    for (let i = 0; i < this.hints.length; i++) this.hints[i] *= k;
    this.vel.fill(0);
  }

  /**
   * 最小间距硬分离：力场收敛后仍贴在一起的节点（通常是同标签的两张卡挤在
   * 锚点同一个方向上）会被推开，否则光点和铭牌会叠成一坨。
   * O(N²)，只在小图上调用。
   */
  separate(minGap: number): void {
    const { pos } = this;
    const n = this.scene.nodes.length;
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        const dx = pos[j * 3] - pos[i * 3];
        const dy = pos[j * 3 + 1] - pos[i * 3 + 1];
        const dz = pos[j * 3 + 2] - pos[i * 3 + 2];
        const d = Math.hypot(dx, dy, dz);
        if (d >= minGap) continue;
        const k = d < 1e-6 ? 0 : ((minGap - d) / 2 / d) * 0.5;
        const ox = d < 1e-6 ? 0.5 : dx * k;
        const oy = d < 1e-6 ? 0.5 : dy * k;
        const oz = d < 1e-6 ? 0 : dz * k;
        pos[i * 3] -= ox; pos[i * 3 + 1] -= oy; pos[i * 3 + 2] -= oz;
        pos[j * 3] += ox; pos[j * 3 + 1] += oy; pos[j * 3 + 2] += oz;
      }
    }
  }
}

/**
 * 跑到收敛（或步数用尽）为止，返回实际步数。
 * 步数上限不是摆设：万级任务收敛要几千步，无上限会让「重新布局」永远转圈；
 * 用尽步数拿到的构图已经足够成图，只是没到数学静止。
 */
export function settle(
  force: Force3D,
  { maxSteps = 900, epsilon = KINETIC_EPSILON } = {},
): number {
  let steps = 0;
  for (; steps < maxSteps; steps++) {
    force.step();
    if (force.kinetic() < epsilon) {
      steps++;
      break;
    }
  }
  return steps;
}
