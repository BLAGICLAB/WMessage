// graphPrefs 单测：默认值 / 读写往返 / 非法值回退 / 过滤器记忆的形状校验与清除语义。
import { beforeEach, describe, expect, it } from "vitest";
import {
  getGraphAutoLayout,
  getGraphEdgeWidth,
  getGraphLabelDensity,
  getGraphLooseness,
  getGraphOnlyMine,
  getGraphRememberFilters,
  getGraphSizeMode,
  loadGraphFilters,
  saveGraphFilters,
  setGraphAutoLayout,
  setGraphEdgeWidth,
  setGraphLabelDensity,
  setGraphLooseness,
  setGraphOnlyMine,
  setGraphRememberFilters,
  setGraphSizeMode,
} from "./graphPrefs";
import {
  DEFAULT_FILTERS,
  type GraphFilters,
} from "../components/GraphPage/graph-build";

describe("graphPrefs", () => {
  beforeEach(() => localStorage.clear());

  it("全部默认值（未写入时）", () => {
    expect(getGraphOnlyMine()).toBe(false);
    expect(getGraphSizeMode()).toBe("degree");
    expect(getGraphLabelDensity()).toBe("standard");
    expect(getGraphEdgeWidth()).toBe("standard");
    expect(getGraphAutoLayout()).toBe(true);
    expect(getGraphLooseness()).toBe("standard");
    expect(getGraphRememberFilters()).toBe(false);
    expect(loadGraphFilters()).toBeNull();
  });

  it("读写往返", () => {
    setGraphOnlyMine(true);
    setGraphSizeMode("duration");
    setGraphLabelDensity("dense");
    setGraphEdgeWidth("thick");
    setGraphAutoLayout(false);
    setGraphLooseness("compact");
    expect(getGraphOnlyMine()).toBe(true);
    expect(getGraphSizeMode()).toBe("duration");
    expect(getGraphLabelDensity()).toBe("dense");
    expect(getGraphEdgeWidth()).toBe("thick");
    expect(getGraphAutoLayout()).toBe(false);
    expect(getGraphLooseness()).toBe("compact");
  });

  it("非法/损坏值回退默认", () => {
    localStorage.setItem("wm.graph.sizeMode", "huge");
    localStorage.setItem("wm.graph.labelDensity", "全部");
    localStorage.setItem("wm.graph.filters", "{not json");
    expect(getGraphSizeMode()).toBe("degree");
    expect(getGraphLabelDensity()).toBe("standard");
    expect(loadGraphFilters()).toBeNull();
  });

  it("过滤器记忆：形状校验 + 关掉开关即清除", () => {
    const f: GraphFilters = {
      ...DEFAULT_FILTERS,
      status: { todo: true, doing: false, done: true },
      owners: [""],
      tags: ["周报"],
      year: 2026,
    };
    saveGraphFilters(f);
    expect(loadGraphFilters()).toEqual(f);
    // 形状损坏（缺 status.done）→ null
    saveGraphFilters(f);
    localStorage.setItem(
      "wm.graph.filters",
      JSON.stringify({
        status: { todo: true, doing: true },
        includeOrphans: true,
      }),
    );
    expect(loadGraphFilters()).toBeNull();
    // 关掉「记住」→ 已存过滤器清除
    saveGraphFilters(f);
    setGraphRememberFilters(true);
    expect(getGraphRememberFilters()).toBe(true);
    setGraphRememberFilters(false);
    expect(getGraphRememberFilters()).toBe(false);
    expect(loadGraphFilters()).toBeNull();
  });
});
