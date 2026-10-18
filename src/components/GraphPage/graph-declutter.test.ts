// 铭牌避让：三维摆位再讲究也躲不开某个机位下的透视压字，这层在屏幕坐标上兜底。
// 这套此前零测试，而避让是 O(M²) 且带预算的——预算一旦写错，表现只是「大图偶尔少几块牌」，
// 没有测试根本看不出来。
import { describe, expect, it } from "vitest";
import { declutterPass, middleBand, placeLabel, type DeclutterTarget } from "./graph-declutter";

const overlapsPanel = (a: { l: number; r: number; t: number; b: number },
  b: { l: number; r: number; t: number; b: number }) =>
  a.l < b.r && a.r > b.l && a.t < b.b && a.b > b.t;

const target = (over: Partial<DeclutterTarget> = {}): DeclutterTarget => ({
  cx: 200,
  cy: 200,
  behind: false,
  w: 60,
  h: 16,
  lift: -16,
  dx: 0,
  dy: 0,
  ...over,
});

const VPORT = { width: 1000, height: 800 };

describe("declutterPass（一帧摆一批铭牌）", () => {
  it("互不重叠的铭牌都原地放下", () => {
    const out = declutterPass(
      [target({ cx: 200 }), target({ cx: 600 })],
      [],
      VPORT.width,
      VPORT.height,
      { budgetMs: 100 },
    );
    expect(out).toEqual([
      { dx: 0, dy: 0, culled: false },
      { dx: 0, dy: 0, culled: false },
    ]);
  });

  it("压上去了就往让：第二块被推到正上方一格，而不是原地叠着", () => {
    const out = declutterPass(
      [target({ cx: 200 }), target({ cx: 205 })],
      [],
      VPORT.width,
      VPORT.height,
      { budgetMs: 100 },
    );
    expect(out[0]).toEqual({ dx: 0, dy: 0, culled: false });
    // 候选顺序：原地 → 上/下四层 → 两侧。第一格是 (0, -(h+5))
    expect(out[1].culled).toBe(false);
    expect(out[1].dx).toBe(0);
    expect(out[1].dy).toBe(-(16 + 5));
  });

  it("相机背后的铭牌直接让掉（那侧根本看不见）", () => {
    const out = declutterPass([target({ behind: true })], [], VPORT.width, VPORT.height, {
      budgetMs: 100,
    });
    expect(out[0]).toEqual({ dx: 0, dy: 0, culled: true });
  });

  it("上次的落位优先复用，旋转时铭牌不跳", () => {
    const out = declutterPass([target({ dx: 80, dy: -40 })], [], VPORT.width, VPORT.height, {
      budgetMs: 100,
    });
    expect(out[0]).toEqual({ dx: 80, dy: -40, culled: false });
  });

  it("仪表面板先占位，铭牌让开它而不是压在面板上", () => {
    const panel = { l: 0, r: 400, t: 0, b: 800 };
    const targets = [target({ cx: 380 }), target({ cx: 395 })];
    const out = declutterPass(targets, [panel], VPORT.width, VPORT.height, {
      budgetMs: 100,
    });
    expect(out.every((r) => !r.culled)).toBe(true);
    for (const [i, r] of out.entries()) {
      const t = targets[i];
      // 落位矩形（placeLabel 里同样的 padding）不得与面板相交
      expect(overlapsPanel({ l: t.cx + r.dx - t.w / 2 - 4, r: t.cx + r.dx + t.w / 2 + 4,
        t: t.cy + r.dy + t.lift - t.h, b: t.cy + r.dy + t.lift }, panel)).toBe(false);
    }
  });

  it("预算耗尽后剩下的铭牌先隐一帧，且保住上次的偏移", () => {
    let calls = 0;
    // 前两次查表给 0（建 deadline + 第一块），之后跳到 100 = 预算已耗尽
    const now = () => (calls++ < 2 ? 0 : 100);
    const targets = Array.from({ length: 20 }, (_, i) => target({ cx: 100 + i * 40, dy: -i }));
    const out = declutterPass(targets, [], VPORT.width, VPORT.height, { budgetMs: 1, now });
    expect(out[0].culled).toBe(false);
    expect(out.slice(1).every((r) => r.culled)).toBe(true);
    expect(out[19]).toEqual({ dx: 0, dy: -19, culled: true });
  });

  it("尺寸还没量到的铭牌由调用方跳过（这里只处理已量到尺寸的）", () => {
    const out = declutterPass([target({ w: 0 })], [], VPORT.width, VPORT.height, {
      budgetMs: 100,
    });
    // 宽 0 意味着 CSS2D 元素还没挂进 DOM；真走到这里会退化成一个点，
    // 所以组件侧先跳过，不把它交给避让
    expect(out).toHaveLength(1);
  });
});

describe("placeLabel", () => {
  it("全部候选位都撞上时返回 null（让调用方隐一帧）", () => {
    const boxes = [{ l: 0, r: 1000, t: 0, b: 800 }];
    const hit = placeLabel(
      { cx: 500, cy: 400, w: 60, h: 16, lift: -16, width: 1000, height: 800, band: { l: 6, r: 994 } },
      boxes,
    );
    expect(hit).toBeNull();
  });
});

describe("middleBand", () => {
  it("半宽以内的面板才把画面切成两半", () => {
    const band = middleBand([{ l: 0, r: 400, t: 0, b: 800 }], 1000);
    expect(band.l).toBe(400);
    expect(band.r).toBe(994);
  });

  it("通栏的顶/底条只压边，不收窄中腰", () => {
    const band = middleBand([{ l: 0, r: 1000, t: 0, b: 60 }], 1000);
    expect(band).toEqual({ l: 6, r: 994 });
  });
});
