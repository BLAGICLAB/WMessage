// format.ts 是事故高发纯逻辑（注释里两次 NaN 历史事故）。
// 行为断言：datetime-local 校验、schedule 四分支解析、月末顺延。
import { describe, it, expect } from "vitest";
import {
  isValidDateTimeLocal,
  scheduleToDatetime,
  formatSchedule,
  formatDue,
  basename,
} from "./format";

describe("isValidDateTimeLocal", () => {
  it("合法值通过", () => {
    expect(isValidDateTimeLocal("2026-09-02T09:30")).toBe(true);
  });

  it("2026-02-31 这类不存在的日期必须拒绝（防 Date 静默进位，历史 NaN 事故）", () => {
    expect(isValidDateTimeLocal("2026-02-31T09:00")).toBe(false);
  });

  it("时分越界拒绝", () => {
    expect(isValidDateTimeLocal("2026-09-02T25:00")).toBe(false);
    expect(isValidDateTimeLocal("2026-09-02T09:99")).toBe(false);
  });

  it("不完整/畸形输入拒绝", () => {
    expect(isValidDateTimeLocal("2026-09-02")).toBe(false);
    expect(isValidDateTimeLocal("2026-9-2T9:0")).toBe(false);
    expect(isValidDateTimeLocal("")).toBe(false);
    expect(isValidDateTimeLocal("not-a-date")).toBe(false);
  });
});

describe("scheduleToDatetime", () => {
  const FALLBACK_RE = /^\d{4}-\d{2}-\d{2}T09:00$/;

  it("空值/未知前缀回退「今天 09:00」", () => {
    expect(scheduleToDatetime(undefined)).toMatch(FALLBACK_RE);
    expect(scheduleToDatetime(null)).toMatch(FALLBACK_RE);
    expect(scheduleToDatetime("garbage")).toMatch(FALLBACK_RE);
  });

  it("at: 一次性规则原样透出（合法时）", () => {
    expect(scheduleToDatetime("at:2026-12-25T18:30")).toBe("2026-12-25T18:30");
  });

  it("at: 坏值回退 fallback，不扩散 NaN", () => {
    expect(scheduleToDatetime("at:not-a-date")).toMatch(FALLBACK_RE);
  });

  it("daily: 解析出今天的 HH:MM", () => {
    expect(scheduleToDatetime("daily:07:05")).toMatch(/T07:05$/);
  });

  it("weekly: 三段解构——分钟不丢（历史 bug：两段拆分分钟 NaN）", () => {
    const out = scheduleToDatetime("weekly:3:14:45");
    expect(out).toMatch(/T14:45$/);
    expect(out).not.toContain("NaN");
  });

  it("weekly: 非法星期回退 fallback", () => {
    expect(scheduleToDatetime("weekly:0:10:00")).toMatch(FALLBACK_RE);
    expect(scheduleToDatetime("weekly:8:10:00")).toMatch(FALLBACK_RE);
  });

  it("monthly: 月末顺延——如 31 日落在无 31 号的月份，逐月顺延不死循环", () => {
    const out = scheduleToDatetime("monthly:31:08:00");
    expect(out).toMatch(/^\d{4}-\d{2}-31T08:00$/);
    expect(out).not.toContain("NaN");
  });

  it("monthly: 非法日回退 fallback", () => {
    expect(scheduleToDatetime("monthly:0:08:00")).toMatch(FALLBACK_RE);
    expect(scheduleToDatetime("monthly:32:08:00")).toMatch(FALLBACK_RE);
  });
});

describe("formatSchedule", () => {
  it("daily/weekly/monthly/at 四分支文案", () => {
    expect(formatSchedule("daily:07:05")).toBe("每天 07:05");
    expect(formatSchedule("weekly:5:18:30")).toBe("每周五 18:30");
    expect(formatSchedule("monthly:15:09:00")).toBe("每月15日 09:00");
    expect(formatSchedule("at:2026-12-25T18:30")).toBe("12-25 18:30 一次");
  });

  it("monthly 坏数据原样显示，不展开 NaN", () => {
    expect(formatSchedule("monthly:0:09:00")).toBe("monthly:0:09:00");
  });
});

describe("formatDue / basename", () => {
  it("due 两种格式（含年份，2026-09-08 老板拍板）", () => {
    expect(formatDue("2026-09-02")).toBe("截止 2026-09-02");
    expect(formatDue("2026-09-02T18:30")).toBe("截止 2026-09-02 18:30");
  });

  it("basename 处理 unix/windows 分隔符", () => {
    expect(basename("/Users/x/report.docx")).toBe("report.docx");
    expect(basename("C:\\Users\\x\\report.docx")).toBe("report.docx");
  });
});
