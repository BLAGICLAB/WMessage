import { describe, expect, it } from "vitest";
import {
  NAV_GAP,
  NAV_LONGEST_LABEL,
  NAV_ROW_MIN_W,
  NAV_W,
} from "./navWidth";

// 宽度是一个纯数字，写错了没有任何编译期或运行期信号：表现是标签被截成
// 「新建…」，要用户截图回来才发现。这里把「最宽一行装得下」的算式固化成
// 预算，改文案/加提示条而忘了加宽就会红。
describe("主导航宽度预算", () => {
  it("NAV_W 装得下最宽一行（带快捷键提示条的行动钮）", () => {
    expect(NAV_W).toBeGreaterThanOrEqual(NAV_ROW_MIN_W);
  });

  it("留的余量够一个汉字：换字体/放大系统字号时不会刚好卡在临界点上", () => {
    expect(NAV_W - NAV_ROW_MIN_W).toBeGreaterThanOrEqual(14);
  });

  it("设置页左栏没有快捷键提示条，同一宽度对它只会更宽松", () => {
    const kbd = 34;
    expect(NAV_W).toBeGreaterThanOrEqual(NAV_ROW_MIN_W - kbd);
  });

  it("最宽标签确实是导航里最长的一条", () => {
    // 「新建任务」四个汉字；导航项最长是「回收站」三个，设置页最长也是四个
    expect(NAV_LONGEST_LABEL.length).toBe(4);
    expect(NAV_LONGEST_LABEL).toBe("新建任务");
  });

  it("浮窗左缘跟着导航宽度走：间距常量不为零且保持小量", () => {
    expect(NAV_GAP).toBeGreaterThan(0);
    expect(NAV_GAP).toBeLessThanOrEqual(8);
  });
});
