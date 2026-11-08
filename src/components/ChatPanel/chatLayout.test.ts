// 聊天区布局尺寸推导链与底栏定档纯函数的契约：
// 全图标行 → 中栏最小宽度 → 窗口最小宽度，一环改了其余要跟着对得上。

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  CHAT_FULL_LAYOUT_MIN_W,
  COMPOSER_ICON_ROW_W,
  COMPOSER_MIN_W,
  labelExtra,
  pickIconLevel,
} from "./chatLayout";

/** vitest 从仓库根起跑：配置文件相对根定位（jsdom 下 import.meta.url 非 file 协议） */
function readTauriConf(): { app: { windows: Array<{ minWidth?: number }> } } {
  return JSON.parse(
    readFileSync("src-tauri/tauri.conf.json", "utf-8"),
  ) as { app: { windows: Array<{ minWidth?: number }> } };
}

describe("pickIconLevel 定档", () => {
  // 门槛 = 全图标行 200 + 未图标化控件的文字额外占宽；三枚都用假文字宽 48
  // （额外占 52），让每一档的边界可以手算
  const e = labelExtra(48);
  const extras = { effort: e, perm: e, model: e };

  it("从 0 档起取第一个放得下的档，边界值落在放得下的一侧", () => {
    expect(pickIconLevel(COMPOSER_ICON_ROW_W + 3 * e, extras)).toBe(0);
    expect(pickIconLevel(COMPOSER_ICON_ROW_W + 3 * e - 1, extras)).toBe(1);
    expect(pickIconLevel(COMPOSER_ICON_ROW_W + 2 * e, extras)).toBe(1);
    expect(pickIconLevel(COMPOSER_ICON_ROW_W + 2 * e - 1, extras)).toBe(2);
    expect(pickIconLevel(COMPOSER_ICON_ROW_W + e, extras)).toBe(2);
    expect(pickIconLevel(COMPOSER_ICON_ROW_W + e - 1, extras)).toBe(3);
  });

  it("连全图标行都不够时落 3 档（中栏下限保证实际不会发生）", () => {
    expect(pickIconLevel(0, extras)).toBe(3);
  });

  it("让位顺序是推理强度 → 授权 → 模型：模型名再长也只在最后一档才图标化", () => {
    // 模型文字很宽、另两枚很窄：只要装得下模型文字就停在 2 档，不提前让模型
    const wide = { effort: 10, perm: 10, model: 300 };
    expect(pickIconLevel(COMPOSER_ICON_ROW_W + 300, wide)).toBe(2);
    expect(pickIconLevel(COMPOSER_ICON_ROW_W + 299, wide)).toBe(3);
  });
});

describe("布局尺寸推导链", () => {
  it("常量快照：全图标行 200 / 中栏下限 222 / 窗口下限 950", () => {
    expect(COMPOSER_ICON_ROW_W).toBe(200);
    expect(COMPOSER_MIN_W).toBe(222);
    expect(CHAT_FULL_LAYOUT_MIN_W).toBe(950);
  });

  it("tauri 窗口 minWidth 与推导链同步（改了布局尺寸没改窗口配置，这条就红）", () => {
    expect(readTauriConf().app.windows[0].minWidth).toBe(
      CHAT_FULL_LAYOUT_MIN_W,
    );
  });
});
