// graph-palette 单测：三套色板 + 语义键 + 唯一取色入口。
// 这一层没有测试就会出「图例色点和节点不是同一个颜色」的事故——
// 画面上只是「图例看着不对」，很难归因到取色函数。
import { describe, expect, it } from "vitest";
import type { GraphNode } from "./graph-build";
import {
  NO_TAG_COLOR,
  OWNER_PALETTE,
  SELF_COLOR,
  STATUS_PALETTE,
  STALL_COLOR,
  STALL_DAYS,
  TAG_PALETTE,
  ownerChipColor,
  ownerColorKey,
  resolveNodeColor,
  tagChipColor,
  tagColorKey,
} from "./graph-palette";
import type { ColorKeys } from "./graph-palette";

function node(partial: Partial<GraphNode> & { id: string }): GraphNode {
  return {
    kind: "task",
    label: partial.id,
    degree: 0,
    ...partial,
  } as GraphNode;
}

describe("ownerColorKey / ownerChipColor", () => {
  it("本人恒为 self，且不进外来成员下标", () => {
    const order = new Map([["p1", 0]]);
    expect(ownerColorKey(node({ id: "a" }), order)).toBe("self");
    expect(ownerColorKey(node({ id: "b", owner: "p1" }), order)).toBe("owner:0");
    expect(ownerChipColor("", order)).toBe(SELF_COLOR);
  });

  it("chip 色点 = 同 ownerKey 的节点色（图例和画布永远同色）", () => {
    const order = new Map([["p1", 1]]);
    const n = node({ id: "b", owner: "p1" });
    expect(resolveNodeColor("owner", {
      ownerKey: ownerColorKey(n, order),
      tagIdx: null,
      statusKey: "todo",
      stalled: false,
    })).toBe(ownerChipColor("p1", order));
  });

  it("成员下标取模，长成员列表不会取到 undefined", () => {
    const long = new Map(
      Array.from({ length: OWNER_PALETTE.length + 3 }, (_, i) => [`p${i}`, i]),
    );
    const color = ownerChipColor("p3", long);
    expect(OWNER_PALETTE).toContain(color);
  });
});

describe("tagColorKey / tagChipColor", () => {
  it("取首个标签；无标签返回 null（走中性色，不占色板位）", () => {
    const order = new Map([["主", 2], ["次", 5]]);
    expect(tagColorKey(node({ id: "a", tags: ["主", "次"] }), order)).toBe(2);
    expect(tagColorKey(node({ id: "b" }), order)).toBeNull();
    expect(resolveNodeColor("tag", {
      ownerKey: "self",
      tagIdx: null,
      statusKey: "todo",
      stalled: false,
    })).toBe(NO_TAG_COLOR);
  });

  it("chip 色点 = 节点色，且色板内无重复色（超过 10 个标签取模，撞色是明知的取舍）", () => {
    expect(new Set(TAG_PALETTE).size).toBe(TAG_PALETTE.length);
    for (let i = 0; i < TAG_PALETTE.length; i++) {
      expect(tagChipColor(i)).toBe(TAG_PALETTE[i]);
      expect(resolveNodeColor("tag", {
        ownerKey: "self",
        tagIdx: i,
        statusKey: "todo",
        stalled: false,
      })).toBe(TAG_PALETTE[i]);
    }
  });
});

describe("resolveNodeColor", () => {
  it("状态模式：进行中/待办/已完成各一色，钉子户单独喊", () => {
    const base = { ownerKey: "self", tagIdx: null, stalled: false };
    expect(resolveNodeColor("status", { ...base, statusKey: "doing" })).toBe(
      STATUS_PALETTE.doing,
    );
    expect(resolveNodeColor("status", { ...base, statusKey: "todo" })).toBe(
      STATUS_PALETTE.todo,
    );
    expect(resolveNodeColor("status", { ...base, statusKey: "done" })).toBe(
      STATUS_PALETTE.done,
    );
    expect(resolveNodeColor("status", { ...base, statusKey: "doing", stalled: true })).toBe(
      STALL_COLOR,
    );
  });

  it("三套色板互不串味：同一节点换模式颜色真的变", () => {
    const k: ColorKeys = { ownerKey: "owner:0", tagIdx: 0, statusKey: "doing", stalled: false };
    const seen = new Set(
      (["tag", "owner", "status"] as const).map((m) => resolveNodeColor(m, k)),
    );
    expect(seen.size).toBe(3);
  });

  it("已完成的低亮石板灰不与「进行中」撞色（完成了不该抢注意力）", () => {
    expect(STATUS_PALETTE.done).not.toBe(STATUS_PALETTE.doing);
  });
});

describe("STALL_DAYS", () => {
  it("14 天，不是随手拍的", () => {
    expect(STALL_DAYS).toBe(14);
  });
});
