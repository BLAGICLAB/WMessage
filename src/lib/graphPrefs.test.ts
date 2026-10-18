// graphPrefs 单测：默认值 / 读写往返 / 非法值回退 / 过滤器记忆的形状校验与清除语义。
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getGraphAutoLayout,
  getGraphEdgeWidth,
  getGraphLabelDensity,
  getGraphLooseness,
  getGraphOnlyMine,
  getGraphRememberFilters,
  loadGraphFilters,
  saveGraphFilters,
  setGraphAutoLayout,
  setGraphEdgeWidth,
  setGraphLabelDensity,
  setGraphLooseness,
  setGraphOnlyMine,
  setGraphRememberFilters,
} from "./graphPrefs";
import {
  DEFAULT_FILTERS,
  type GraphFilters,
} from "../components/GraphPage/graph-build";

describe("graphPrefs", () => {
  beforeEach(() => localStorage.clear());

  it("全部默认值（未写入时）", () => {
    expect(getGraphOnlyMine()).toBe(false);
    expect(getGraphLabelDensity()).toBe("standard");
    expect(getGraphEdgeWidth()).toBe("standard");
    expect(getGraphAutoLayout()).toBe(true);
    expect(getGraphLooseness()).toBe("standard");
    expect(getGraphRememberFilters()).toBe(false);
    expect(loadGraphFilters()).toBeNull();
  });

  it("读写往返", () => {
    setGraphOnlyMine(true);
    setGraphLabelDensity("dense");
    setGraphEdgeWidth("thick");
    setGraphAutoLayout(false);
    setGraphLooseness("tight");
    expect(getGraphOnlyMine()).toBe(true);
    expect(getGraphLabelDensity()).toBe("dense");
    expect(getGraphEdgeWidth()).toBe("thick");
    expect(getGraphAutoLayout()).toBe(false);
    expect(getGraphLooseness()).toBe("tight");
  });

  it("旧三档存档搬一次：原紧凑/标准 → 新紧凑，原松散 → 新标准", () => {
    // 标准档的 k 从 30 变成 45，同名不同义，只能靠换键区分，不能就地改写
    localStorage.setItem("wm.graph.looseness", "standard");
    expect(getGraphLooseness()).toBe("tight");
    expect(localStorage.getItem("wm.graph.looseness.v2")).toBe("tight");
    expect(localStorage.getItem("wm.graph.looseness")).toBeNull();

    localStorage.clear();
    localStorage.setItem("wm.graph.looseness", "loose");
    expect(getGraphLooseness()).toBe("standard");

    localStorage.clear();
    localStorage.setItem("wm.graph.looseness", "compact");
    expect(getGraphLooseness()).toBe("tight");
  });

  it("旧存档不认识时不猜：回默认；新键存在时旧键一律不看", () => {
    localStorage.setItem("wm.graph.looseness", "松散");
    expect(getGraphLooseness()).toBe("standard");
    localStorage.setItem("wm.graph.looseness.v2", "loose");
    expect(getGraphLooseness()).toBe("loose");
  });

  it("搬迁写失败（配额满）时保留旧存档，下次再搬", () => {
    localStorage.setItem("wm.graph.looseness", "loose");
    const spy = vi
      .spyOn(window.localStorage, "setItem")
      .mockImplementation(() => {
        throw new DOMException("quota", "QuotaExceededError");
      });
    expect(getGraphLooseness()).toBe("standard");
    expect(localStorage.getItem("wm.graph.looseness")).toBe("loose");
    spy.mockRestore();
    expect(getGraphLooseness()).toBe("standard");
    expect(localStorage.getItem("wm.graph.looseness.v2")).toBe("standard");
    expect(localStorage.getItem("wm.graph.looseness")).toBeNull();
  });

  it("非法/损坏值回退默认", () => {
    localStorage.setItem("wm.graph.labelDensity", "全部");
    localStorage.setItem("wm.graph.filters", "{not json");
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
