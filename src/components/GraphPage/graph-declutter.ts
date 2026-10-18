// 铭牌避让：三维摆位再讲究也躲不开某个机位下的透视压字，这层在屏幕坐标上兜底。
//
// 坑（早前踩过）：只做「往下一格让」，一旦让到的高度恰好等于下方那块的下沿，
// dy 就再也不增长，卡在同一个值上死循环——半个场景的铭牌就这么压在一起。
// 现在是候选位贪心：上次的偏移排第一个（旋转时不抖），再依次试上下左右与两侧，
// 都不行才横着推进中腰空档，仍放不下就整块让掉。压字比缺牌难看。

export interface Rect {
  l: number;
  r: number;
  t: number;
  b: number;
}

export interface DeclutterInput {
  /** 节点投影到屏幕后的中心 */
  cx: number;
  cy: number;
  /** 铭牌尺寸 */
  w: number;
  h: number;
  /** 铭牌相对节点的竖向抬升（放在光点上方，所以是负的） */
  lift: number;
  /** 视口 */
  width: number;
  height: number;
  /** 中腰没被面板占掉的横带 */
  band: { l: number; r: number };
}

const EDGE = 6;
const PAD = 4;

const overlaps = (a: Rect, b: Rect) => a.l < b.r && a.r > b.l && a.t < b.b && a.b > b.t;

/**
 * 给一块铭牌找落位。命中就把矩形推进 `boxes`（调用方负责复用数组），
 * 真没地方了返回 null——调用方据此把铭牌藏一帧。
 */
export function placeLabel(
  input: DeclutterInput,
  boxes: Rect[],
  prev?: { dx: number; dy: number },
): { dx: number; dy: number } | null {
  const { cx, cy, w, h, lift, width, height, band } = input;
  const gap = h + 5;
  const side = w / 2 + 12;
  // 顺序即优先级：先原地（带上次的偏移，旋转时不抖），再上下四层，最后往两侧。
  // 右侧先于左侧——左边通常压着仪器栏，让过去等于让进面板底下。
  const cand: Array<[number, number]> = [];
  // prev 正好是原点时不要再补一个 (0,0)：那是同一个位置，
  // 上一次判定失败的话这次必然同样失败，等于每帧白试一轮
  if (prev && (prev.dx !== 0 || prev.dy !== 0)) cand.push([prev.dx, prev.dy]);
  cand.push([0, 0]);
  for (let k = 1; k <= 4; k++) cand.push([0, -k * gap], [0, k * gap]);
  for (let k = 1; k <= 3; k++) {
    cand.push(
      [side * k, 0],
      [side * k, -gap],
      [side * k, gap],
      [-side * k, 0],
      [-side * k, -gap],
      [-side * k, gap],
    );
  }
  const tryAt = (dx: number, dy: number): boolean => {
    const box = {
      l: cx + dx - w / 2 - PAD,
      r: cx + dx + w / 2 + PAD,
      t: cy + dy + lift - h,
      b: cy + dy + lift,
    };
    if (box.l < EDGE || box.r > width - EDGE || box.t < EDGE || box.b > height - EDGE) {
      return false;
    }
    for (const o of boxes) if (overlaps(box, o)) return false;
    boxes.push(box);
    return true;
  };
  for (const [dx, dy] of cand) {
    if (tryAt(dx, dy)) return { dx, dy };
  }
  // 候选位挤不下就别塞回原位：横着推进中腰那条没被面板占掉的空档
  if (band.r - band.l > w) {
    const dx =
      cx + w / 2 > band.r
        ? band.r - w / 2 - cx
        : cx - w / 2 < band.l
          ? band.l - w / 2 - cx
          : 0;
    if (tryAt(dx, 0)) return { dx, dy: 0 };
  }
  return null;
}

/**
 * 一帧里要摆位的铭牌（屏幕坐标 + 尺寸 + 上次落位）。
 * 尺寸为 0 表示元素还没被 CSS2DRenderer 挂进 DOM，量不出来——调用方跳过它。
 */
export interface DeclutterTarget {
  cx: number;
  cy: number;
  /** 投影 z > 1：在相机背后，那侧的铭牌看不见 */
  behind: boolean;
  w: number;
  h: number;
  lift: number;
  dx: number;
  dy: number;
}

export interface DeclutterOutcome {
  dx: number;
  dy: number;
  /** 这一帧藏起来（相机背后 / 真挤不下 / 预算耗尽） */
  culled: boolean;
}

export interface DeclutterOptions {
  /**
   * 一帧最多花多少毫秒摆位。摆位是 O(M²)（每块都要和已占位比对），万级图上一帧
   * 能吃掉 68ms——预算耗尽后剩下的铭牌这一帧先让掉，下一帧接着摆。
   * 压字比缺牌难看，卡住整帧比压字更难看。
   */
  budgetMs: number;
  /** 注入时钟，单测用 */
  now?: () => number;
}

/**
 * 一批铭牌一次摆完，结果与 `targets` 一一对应。
 * 障碍物（仪表面板等）先占位，中腰空带由它们算出。
 */
export function declutterPass(
  targets: DeclutterTarget[],
  obstacles: Rect[],
  width: number,
  height: number,
  options: DeclutterOptions,
): DeclutterOutcome[] {
  const now = options.now ?? (() => performance.now());
  const deadline = now() + options.budgetMs;
  const boxes: Rect[] = [...obstacles];
  const band = middleBand(obstacles, width);
  const out: DeclutterOutcome[] = [];
  for (const t of targets) {
    if (now() >= deadline) {
      // 预算用完：保住上次的偏移，只让这一帧，下一帧预算恢复再摆回去
      out.push({ dx: t.dx, dy: t.dy, culled: true });
      continue;
    }
    if (t.behind) {
      out.push({ dx: 0, dy: 0, culled: true });
      continue;
    }
    const hit = placeLabel(
      { cx: t.cx, cy: t.cy, w: t.w, h: t.h, lift: t.lift, width, height, band },
      boxes,
      { dx: t.dx, dy: t.dy },
    );
    // 真没地方了：藏这一帧。挤在一起的两块字没人读得下去，缺一块还有人找得到
    out.push(hit ? { dx: hit.dx, dy: hit.dy, culled: false } : { dx: 0, dy: 0, culled: true });
  }
  return out;
}

/**
 * 仪表面板的中腰空带：通栏的顶/底条只压边不算障碍，只有半宽以内的面板
 * 才真的把画面切成两半。
 */
export function middleBand(
  rails: Rect[],
  width: number,
): { l: number; r: number } {
  const band = { l: EDGE, r: width - EDGE };
  for (const o of rails) {
    if (o.r - o.l > width * 0.8) continue;
    if (o.l < width / 2) band.l = Math.max(band.l, o.r);
    else band.r = Math.min(band.r, o.l);
  }
  return band;
}
