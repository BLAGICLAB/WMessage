// bubbleStyle 单测：存储归一 / DOM 应用（CSS 变量 + data 属性）/ 文字色自动反白 /
// storage 事件跨窗口同步。localStorage 走 src/test/setup.ts 的内存 shim。

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  applyBubbleStyle,
  applyBubbleStyleDom,
  BUBBLE_STYLE_DEFAULT,
  getBubbleStyle,
  pickUserFg,
  resetBubbleStyle,
  subscribeBubbleStyle,
} from "./bubbleStyle";

const KEY = "wmessage-bubble-style";

beforeEach(() => {
  localStorage.clear();
  document.documentElement.style.removeProperty("--bubble-user-bg");
  document.documentElement.style.removeProperty("--bubble-user-fg");
  delete document.documentElement.dataset.bubbleBot;
});

describe("getBubbleStyle", () => {
  it("无存储时回退默认（跟随主题 + 卡片材质）", () => {
    expect(getBubbleStyle()).toEqual(BUBBLE_STYLE_DEFAULT);
  });

  it("脏数据（非法 JSON / 非法颜色 / 非法材质）逐字段归一，不抛错", () => {
    localStorage.setItem(KEY, "{broken json");
    expect(getBubbleStyle()).toEqual(BUBBLE_STYLE_DEFAULT);

    localStorage.setItem(
      KEY,
      JSON.stringify({ userBg: "red", botMaterial: "glass" }),
    );
    expect(getBubbleStyle()).toEqual(BUBBLE_STYLE_DEFAULT);
  });

  it("合法存储原样读回", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify({ userBg: "#3563b0", botMaterial: "flat" }),
    );
    expect(getBubbleStyle()).toEqual({ userBg: "#3563b0", botMaterial: "flat" });
  });
});

describe("pickUserFg", () => {
  it("深底色（含品牌色 #5f6f94）配白字，浅底色配深字", () => {
    expect(pickUserFg("#5f6f94")).toBe("#ffffff");
    expect(pickUserFg("#3563b0")).toBe("#ffffff");
    expect(pickUserFg("#e8b4b4")).toBe("#171d2b");
    expect(pickUserFg("not-a-color")).toBe("#ffffff");
  });
});

describe("applyBubbleStyle", () => {
  it("自定义颜色：写 CSS 变量 + data 属性 + 落盘；白字配白透芯片", () => {
    const s = applyBubbleStyle({ userBg: "#3563b0", botMaterial: "inset" });
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--bubble-user-bg")).toBe("#3563b0");
    expect(root.style.getPropertyValue("--bubble-user-fg")).toBe("#ffffff");
    expect(root.style.getPropertyValue("--bubble-chip-bg")).toBe(
      "rgba(255, 255, 255, 0.16)",
    );
    expect(root.dataset.bubbleBot).toBe("inset");
    expect(JSON.parse(localStorage.getItem(KEY)!)).toEqual(s);
  });

  it("浅色自定义底色：文字/芯片自动换深色系", () => {
    applyBubbleStyle({ userBg: "#e8b4b4", botMaterial: "card" });
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--bubble-user-fg")).toBe("#171d2b");
    expect(root.style.getPropertyValue("--bubble-chip-fg")).toBe("#26304a");
    expect(root.style.getPropertyValue("--bubble-chip-bg")).toBe(
      "rgba(23, 32, 51, 0.1)",
    );
  });

  it("恢复默认：清掉全部 CSS 变量，data 属性回卡片", () => {
    applyBubbleStyle({ userBg: "#3563b0", botMaterial: "flat" });
    resetBubbleStyle();
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--bubble-user-bg")).toBe("");
    expect(root.style.getPropertyValue("--bubble-chip-fg")).toBe("");
    expect(root.dataset.bubbleBot).toBe("card");
    expect(getBubbleStyle()).toEqual(BUBBLE_STYLE_DEFAULT);
  });

  it("applyBubbleStyleDom 只动 DOM 不落盘（订阅路径防 storage 回声）", () => {
    applyBubbleStyleDom({ userBg: "#6d5aa8", botMaterial: "card" });
    expect(document.documentElement.style.getPropertyValue("--bubble-user-bg")).toBe(
      "#6d5aa8",
    );
    expect(localStorage.getItem(KEY)).toBeNull();
  });
});

describe("subscribeBubbleStyle", () => {
  it("storage 事件（本 key）：同步 DOM 并回调解析后的样式", () => {
    const onChange = vi.fn();
    const unsub = subscribeBubbleStyle(onChange);
    localStorage.setItem(
      KEY,
      JSON.stringify({ userBg: "#b04a6a", botMaterial: "flat" }),
    );
    window.dispatchEvent(
      new StorageEvent("storage", {
        key: KEY,
        newValue: localStorage.getItem(KEY),
      }),
    );
    expect(onChange).toHaveBeenCalledWith({ userBg: "#b04a6a", botMaterial: "flat" });
    expect(document.documentElement.dataset.bubbleBot).toBe("flat");

    unsub();
    window.dispatchEvent(
      new StorageEvent("storage", { key: KEY, newValue: "{}" }),
    );
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("其他 key 的 storage 事件不触发", () => {
    const onChange = vi.fn();
    subscribeBubbleStyle(onChange);
    window.dispatchEvent(
      new StorageEvent("storage", { key: "wmessage-theme", newValue: "dark" }),
    );
    expect(onChange).not.toHaveBeenCalled();
  });
});
