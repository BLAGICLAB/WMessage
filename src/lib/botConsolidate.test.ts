// 标签整理的前后端常量对拍：占位值与长度上限必须两侧一致，
// 否则前端拦得住的输入后端放行（或反过来），错误只在实机上现形。
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
  NEW_MAIN_PLACEHOLDER,
  MAX_MAIN_TAG_LEN,
} from "./botConsolidate";

const rustSrc = readFileSync(
  resolve(__dirname, "../../src-tauri/src/bot/tags_consolidate.rs"),
  "utf-8",
);
const rustTypesSrc = readFileSync(
  resolve(__dirname, "../../src-tauri/src/bot/config/types.rs"),
  "utf-8",
);

describe("标签整理的前后端常量对拍", () => {
  it("占位值与后端 NEW_MAIN_PLACEHOLDER 一致", () => {
    const m = rustSrc.match(
      /pub const NEW_MAIN_PLACEHOLDER:\s*&str\s*=\s*"([^"]+)"/,
    );
    expect(m, "tags_consolidate.rs 里应能找到 NEW_MAIN_PLACEHOLDER 定义").toBeTruthy();
    expect(m![1]).toBe(NEW_MAIN_PLACEHOLDER);
  });

  it("长度上限与后端任务标签口径一致（MAX_TAG_LEN）", () => {
    const m = rustTypesSrc.match(/pub const MAX_TAG_LEN:\s*usize\s*=\s*(\d+)/);
    expect(m, "types.rs 里应能找到 MAX_TAG_LEN 定义").toBeTruthy();
    expect(Number(m![1])).toBe(MAX_MAIN_TAG_LEN);
  });
});
