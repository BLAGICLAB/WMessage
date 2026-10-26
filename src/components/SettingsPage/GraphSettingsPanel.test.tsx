// GraphSettingsPanel 单测：六项偏好渲染 + 开关/三档写入 localStorage（graphPrefs 键）。
import { beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";
import GraphSettingsPanel from "./GraphSettingsPanel";

describe("GraphSettingsPanel", () => {
  beforeEach(() => localStorage.clear());

  it("六项设置全部渲染，默认值正确", () => {
    render(<GraphSettingsPanel />);
    expect(
      screen.getByRole("switch", { name: "只看我的任务" }),
    ).toHaveAttribute("aria-checked", "false");
    expect(
      screen.getByRole("switch", { name: "打开时自动播放布局动画" }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("switch", { name: "记住上次的过滤器" }),
    ).toHaveAttribute("aria-checked", "false");
    expect(
      screen.getByRole("radiogroup", { name: "标签密度" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("radiogroup", { name: "连线粗细" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("radiogroup", { name: "星点大小" }),
    ).toBeInTheDocument();
  });

  it("开关与三档切换即时写入 localStorage", () => {
    render(<GraphSettingsPanel />);
    fireEvent.click(screen.getByRole("switch", { name: "只看我的任务" }));
    expect(localStorage.getItem("wm.graph.onlyMine")).toBe("1");
    fireEvent.click(
      screen.getByRole("switch", { name: "打开时自动播放布局动画" }),
    );
    expect(localStorage.getItem("wm.graph.autoLayout")).toBe("0");

    const density = screen.getByRole("radiogroup", { name: "标签密度" });
    fireEvent.click(within(density).getByRole("radio", { name: "多" }));
    expect(localStorage.getItem("wm.graph.labelDensity")).toBe("dense");

    const loose = screen.getByRole("radiogroup", { name: "星点大小" });
    fireEvent.click(within(loose).getByRole("radio", { name: "紧凑" }));
    expect(localStorage.getItem("wm.graph.looseness.v2")).toBe("tight");
  });

  it("记住过滤器：关 → 开 → 关 后已存过滤器被清除", () => {
    localStorage.setItem(
      "wm.graph.filters",
      '{"status":{"todo":true,"doing":true,"done":true},"includeOrphans":true}',
    );
    render(<GraphSettingsPanel />);
    const sw = screen.getByRole("switch", { name: "记住上次的过滤器" });
    fireEvent.click(sw); // 开
    expect(localStorage.getItem("wm.graph.rememberFilters")).toBe("1");
    fireEvent.click(sw); // 关 → 清除存档
    expect(localStorage.getItem("wm.graph.rememberFilters")).toBe("0");
    expect(localStorage.getItem("wm.graph.filters")).toBeNull();
  });

  it("回读持久化值渲染初始态", () => {
    localStorage.setItem("wm.graph.edgeWidth", "thick");
    render(<GraphSettingsPanel />);
    const edge = screen.getByRole("radiogroup", { name: "连线粗细" });
    expect(within(edge).getByRole("radio", { name: "粗" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  it("「节点大小」整组已退役：节点大小只由连接度决定，不给开关", () => {
    render(<GraphSettingsPanel />);
    expect(screen.queryByRole("radiogroup", { name: "节点大小" })).toBeNull();
    expect(screen.queryByRole("radio", { name: "耗时" })).toBeNull();
  });

  it("三档标签齐全：密度 少/标准/多，粗细 细/标准/粗，松散度 紧凑/标准/宽松", () => {
    render(<GraphSettingsPanel />);
    const labels = (name: string) =>
      within(screen.getByRole("radiogroup", { name }))
        .getAllByRole("radio")
        .map((el) => el.textContent);
    expect(labels("标签密度")).toEqual(["少", "标准", "多"]);
    expect(labels("连线粗细")).toEqual(["细", "标准", "粗"]);
    expect(labels("星点大小")).toEqual(["紧凑", "标准", "宽松"]);
  });

  it("文案里的关键口径不被改掉：度数分位、三档像素值、松散度调的是星点大小", () => {
    render(<GraphSettingsPanel />);
    const text = document.body.textContent ?? "";
    // 标签密度：按度数分位取门槛（写死阈值会让密度档随图的疏密漂移）
    expect(text).toContain("度数前 7%");
    // 连线粗细：三档的名义像素值
    expect(text).toContain("细 1px / 标准 1.6px / 粗 2.8px");
    // 星点大小：调的是星点大小，不改节点排布
    expect(text).toContain("星点的大小与亮暗");
    expect(text).toContain("不改节点之间的排布");
  });
});
