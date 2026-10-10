import { describe, expect, it } from "vitest";
import {
  DAY_END_MIN,
  DAY_START_MIN,
  DEFAULT_DURATION_MIN,
  DAY_SPAN_MIN,
  SNAP_MIN,
  addDays,
  buildPlanDT,
  clampWindowMin,
  fmtMin,
  layoutLanes,
  parsePlanDT,
  planChip,
  planColorVar,
  segsForWeek,
  snapMin,
  startOfWeek,
} from "./week";

const MON = new Date(2026, 9, 5); // 2026-10-05 周一
const WED = new Date(2026, 9, 7); // 周三

describe("startOfWeek / 日期工具", () => {
  it("周日回退到本周一，周一保持不变，均归零到 00:00", () => {
    const sun = new Date(2026, 9, 11, 15, 30); // 周六? 2026-10-11 是周日
    const s = startOfWeek(sun);
    expect(s.getFullYear()).toBe(2026);
    expect(s.getMonth()).toBe(9);
    expect(s.getDate()).toBe(5);
    expect(s.getHours()).toBe(0);
    expect(s.getMinutes()).toBe(0);
    const m = startOfWeek(MON);
    expect(m.getDate()).toBe(5);
  });

  it("addDays 跨月进位正确", () => {
    const d = addDays(new Date(2026, 9, 30), 3);
    expect([d.getMonth(), d.getDate()]).toEqual([10, 2]);
  });
});

describe("吸附与格式化", () => {
  it("snapMin 就近取整到 30 分钟", () => {
    expect(snapMin(9 * 60 + 5)).toBe(9 * 60);
    expect(snapMin(9 * 60 + 20)).toBe(9 * 60 + 30);
  });

  it("clampWindowMin 夹进 [8:00, 17:30]", () => {
    expect(clampWindowMin(7 * 60)).toBe(DAY_START_MIN);
    expect(clampWindowMin(17 * 60 + 45)).toBe(DAY_END_MIN - SNAP_MIN);
  });

  it("fmtMin 补零", () => {
    expect(fmtMin(9 * 60 + 5)).toBe("09:05");
    expect(fmtMin(18 * 60)).toBe("18:00");
  });

  it("parsePlanDT 接受合法值、拒绝错格式/越界字段/不存在的日期", () => {
    expect(parsePlanDT("2026-10-07T09:30")).toEqual({ day: WED, min: 570 });
    expect(parsePlanDT("明天下午")).toBeNull();
    expect(parsePlanDT("2026-10-07")).toBeNull();
    expect(parsePlanDT("2026-13-01T10:00")).toBeNull();
    expect(parsePlanDT("2026-10-07T09:70")).toBeNull();
    expect(parsePlanDT("2026-02-30T10:00")).toBeNull();
  });

  it("buildPlanDT 与 parsePlanDT 往返一致", () => {
    const s = "2026-10-07T17:05";
    const p = parsePlanDT(s);
    expect(p && buildPlanDT(p.day, p.min)).toBe(s);
  });
});

describe("segsForWeek 跨日分段与窗口裁剪", () => {
  it("同日计划产出单段，无续接标记", () => {
    const segs = segsForWeek("2026-10-07T10:00", "2026-10-07T11:00", MON);
    expect(segs).toEqual([
      { dayIdx: 2, startMin: 600, endMin: 660, head: false, tail: false },
    ]);
  });

  it("溢出 18:00 折入次日：跨午夜的计划在当日贴边段 tail + 次日列顶段 head（写入侧 UI 负责把 end 折到次日物理时间）", () => {
    const segs = segsForWeek("2026-10-07T17:30", "2026-10-08T08:30", MON);
    expect(segs).toEqual([
      { dayIdx: 2, startMin: 17 * 60 + 30, endMin: DAY_END_MIN, head: false, tail: true },
      { dayIdx: 3, startMin: DAY_START_MIN, endMin: 8 * 60 + 30, head: true, tail: false },
    ]);
  });

  it("8:00 之前开始的既有数据：列顶截断 + head", () => {
    const segs = segsForWeek("2026-10-07T07:00", "2026-10-07T09:00", MON);
    expect(segs).toEqual([
      { dayIdx: 2, startMin: DAY_START_MIN, endMin: 9 * 60, head: true, tail: false },
    ]);
  });

  it("跨多日的长计划逐日产出分段", () => {
    const segs = segsForWeek("2026-10-06T17:00", "2026-10-08T10:00", MON);
    expect(segs.map((s) => s.dayIdx)).toEqual([1, 2, 3]);
    expect(segs[0].tail).toBe(true);
    expect(segs[1].head && segs[1].tail).toBe(true);
    expect(segs[2].head).toBe(true);
  });

  it("跨周部分不产出分段（周日后半段在下一周）", () => {
    const segs = segsForWeek("2026-10-11T17:30", "2026-10-12T09:00", MON);
    expect(segs).toHaveLength(1);
    expect(segs[0].dayIdx).toBe(6);
    expect(segs[0].tail).toBe(true);
  });

  it("完全落在 8-18 窗口外：产出近端 30 分钟残段（双向续接标记）", () => {
    const segs = segsForWeek("2026-10-07T20:00", "2026-10-07T21:00", MON);
    expect(segs).toEqual([
      { dayIdx: 2, startMin: DAY_END_MIN - SNAP_MIN, endMin: DAY_END_MIN, head: true, tail: true },
    ]);
  });

  it("planEnd <= planStart 的脏数据返空（不渲染）", () => {
    expect(segsForWeek("2026-10-07T11:00", "2026-10-07T10:00", MON)).toEqual([]);
  });
});

describe("layoutLanes 重叠分栏", () => {
  it("两段重叠 → 同簇两栏；不相交 → 各占一栏（lanes=1）", () => {
    const out = layoutLanes([
      { startMin: 540, endMin: 630 },
      { startMin: 540, endMin: 630 },
      { startMin: 800, endMin: 860 },
    ]);
    expect(out.map((o) => [o.lane, o.lanes])).toEqual([
      [0, 2],
      [1, 2],
      [0, 1],
    ]);
  });

  it("链式重叠（A-B-C 两两相接重叠）传递成同簇", () => {
    const out = layoutLanes([
      { startMin: 540, endMin: 600 },
      { startMin: 570, endMin: 630 },
      { startMin: 600, endMin: 660 },
    ]);
    expect(out.every((o) => o.lanes === 2)).toBe(true);
  });
});

describe("颜色与角标", () => {
  it("无标签 → slate；同标签恒同色；不同标签可不同色", () => {
    expect(planColorVar(undefined)).toBe("var(--plan-slate)");
    expect(planColorVar([])).toBe("var(--plan-slate)");
    expect(planColorVar(["设计"])).toBe(planColorVar(["设计"]));
  });

  it("planChip 输出 周几 + HH:mm", () => {
    expect(planChip("2026-10-07T09:30")).toBe("三 09:30");
    expect(planChip("2026-10-07T10:00")).toBe("三 10:00");
    expect(planChip("垃圾")).toBe("");
  });
});

describe("常量契约", () => {
  it("窗口/吸附/默认时长", () => {
    expect(DAY_SPAN_MIN).toBe(600);
    expect(DEFAULT_DURATION_MIN).toBe(60);
    expect(SNAP_MIN).toBe(30);
  });
});
