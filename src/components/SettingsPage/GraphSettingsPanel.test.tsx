// GraphSettingsPanel 单测：七项偏好渲染 + 开关/三档写入 localStorage（graphPrefs 键）。
import { beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";
import GraphSettingsPanel from "./GraphSettingsPanel";

describe("GraphSettingsPanel", () => {
  beforeEach(() => localStorage.clear());

  it("七项设置全部渲染，默认值正确", () => {
    render(<GraphSettingsPanel />);
    expect(screen.getByRole("switch", { name: "只看我的任务" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("switch", { name: "打开时自动播放布局动画" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "记住上次的过滤器" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("radiogroup", { name: "节点大小" })).toBeInTheDocument();
    expect(screen.getByRole("radiogroup", { name: "标签密度" })).toBeInTheDocument();
    expect(screen.getByRole("radiogroup", { name: "连线粗细" })).toBeInTheDocument();
    expect(screen.getByRole("radiogroup", { name: "布局松散度" })).toBeInTheDocument();
  });

  it("开关与三档切换即时写入 localStorage", () => {
    render(<GraphSettingsPanel />);
    fireEvent.click(screen.getByRole("switch", { name: "只看我的任务" }));
    expect(localStorage.getItem("wm.graph.onlyMine")).toBe("1");
    fireEvent.click(screen.getByRole("switch", { name: "打开时自动播放布局动画" }));
    expect(localStorage.getItem("wm.graph.autoLayout")).toBe("0");

    const density = screen.getByRole("radiogroup", { name: "标签密度" });
    fireEvent.click(within(density).getByRole("radio", { name: "多" }));
    expect(localStorage.getItem("wm.graph.labelDensity")).toBe("dense");

    const size = screen.getByRole("radiogroup", { name: "节点大小" });
    fireEvent.click(within(size).getByRole("radio", { name: "耗时" }));
    expect(localStorage.getItem("wm.graph.sizeMode")).toBe("duration");

    const loose = screen.getByRole("radiogroup", { name: "布局松散度" });
    fireEvent.click(within(loose).getByRole("radio", { name: "紧凑" }));
    expect(localStorage.getItem("wm.graph.looseness")).toBe("compact");
  });

  it("记住过滤器：关 → 开 → 关 后已存过滤器被清除", () => {
    localStorage.setItem("wm.graph.filters", '{"status":{"todo":true,"doing":true,"done":true},"includeOrphans":true}');
    render(<GraphSettingsPanel />);
    const sw = screen.getByRole("switch", { name: "记住上次的过滤器" });
    fireEvent.click(sw); // 开
    expect(localStorage.getItem("wm.graph.rememberFilters")).toBe("1");
    fireEvent.click(sw); // 关 → 清除存档
    expect(localStorage.getItem("wm.graph.rememberFilters")).toBe("0");
    expect(localStorage.getItem("wm.graph.filters")).toBeNull();
  });

  it("回读持久化值渲染初始态", () => {
    localStorage.setItem("wm.graph.sizeMode", "duration");
    localStorage.setItem("wm.graph.edgeWidth", "thick");
    render(<GraphSettingsPanel />);
    const size = screen.getByRole("radiogroup", { name: "节点大小" });
    expect(within(size).getByRole("radio", { name: "耗时" })).toHaveAttribute("aria-checked", "true");
    const edge = screen.getByRole("radiogroup", { name: "连线粗细" });
    expect(within(edge).getByRole("radio", { name: "粗" })).toHaveAttribute("aria-checked", "true");
  });
});
