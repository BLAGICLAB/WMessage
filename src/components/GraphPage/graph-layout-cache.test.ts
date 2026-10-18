// 图谱构图的复用判据：改一条依赖只是换了几条边，节点集合与外径都没变。
// 曾经每次任务写入都把整场拆掉重建——相机弹回定稿机位、整片星云从种子重排，
// 数据没大动画面却整个重来。判据必须严：换了松散度、增删任务、动过滤器都该重来。
import { describe, expect, it } from "vitest";
import { canReuseLayout } from "./GraphCanvas";

const cache = (ids: string[], radius = 450) => ({
  ids,
  radius,
  pos: new Float32Array(ids.length * 3),
  camera: [1, 2, 3] as [number, number, number],
  target: [0, 0, 0] as [number, number, number],
});

const IDS = ["hub:owner:a", "t1", "t2", "t3"];

describe("构图复用判据 canReuseLayout", () => {
  it("同一批节点、同一外径 → 接着上次走", () => {
    expect(canReuseLayout(cache(IDS), [...IDS], 450)).toBe(true);
  });

  it("换个节点集合就重来（增删任务、动过滤器）", () => {
    expect(canReuseLayout(cache(IDS), [...IDS, "t4"], 450)).toBe(false);
    expect(canReuseLayout(cache(IDS), ["t1", "t2", "t3"], 450)).toBe(false);
  });

  it("外径变了就重来（松散度换档）", () => {
    expect(canReuseLayout(cache(IDS, 450), [...IDS], 700)).toBe(false);
    // R = k·√N 是确定算出来的：同一份输入必须逐位相同才敢复用，差一丝也不
    expect(canReuseLayout(cache(IDS, 450), [...IDS], 450 * (1 + 1e-9))).toBe(false);
  });

  it("节点顺序变了就不复用（构图按下标一一对应，错位等于串台）", () => {
    expect(canReuseLayout(cache(IDS), ["t1", "hub:owner:a", "t2", "t3"], 450)).toBe(false);
  });

  it("没有缓存时从零开始", () => {
    expect(canReuseLayout(null, [...IDS], 450)).toBe(false);
  });
});
