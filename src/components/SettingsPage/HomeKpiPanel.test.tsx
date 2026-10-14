import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { HomeKpiPanel } from "./HomeKpiPanel";
import { getHomeKpiSelection } from "../../lib/homeKpi";

beforeEach(() => {
  localStorage.clear();
});

describe("HomeKpiPanel", () => {
  it("8 项口径全量列出，默认选中前 4 并带位置标号", () => {
    render(<HomeKpiPanel />);
    expect(screen.getByLabelText("任务完成率")).toBeChecked();
    expect(screen.getByLabelText("大型任务数")).toBeChecked();
    expect(screen.getByLabelText("逾期任务数")).not.toBeChecked();
    expect(screen.getByLabelText("逾期任务数")).toBeDisabled(); // 已满 4
    expect(screen.getByText("第 1")).toBeInTheDocument();
    expect(screen.getByText("第 4")).toBeInTheDocument();
    expect(screen.getByText("已选 4/4，顺序即首页展示顺序；改动即时生效并自动保存。"))
      .toBeInTheDocument();
  });

  it("先取消一项再补选：落盘顺序 = 原序 + 追加", () => {
    render(<HomeKpiPanel />);
    fireEvent.click(screen.getByLabelText("任务完成率")); // 取消
    expect(screen.getByLabelText("按期完成率")).toBeEnabled();
    fireEvent.click(screen.getByLabelText("按期完成率"));
    expect(getHomeKpiSelection()).toEqual([
      "largeCount",
      "largeRatio",
      "largeDoneRate",
      "onTimeRate",
    ]);
  });

  it("↑↓ 调序即展示顺序，实时落盘", () => {
    render(<HomeKpiPanel />);
    fireEvent.click(screen.getByLabelText("上移大型任务占比"));
    expect(getHomeKpiSelection()).toEqual([
      "completionRate",
      "largeRatio",
      "largeCount",
      "largeDoneRate",
    ]);
    // 顶部不能再上移 / 底部不能再下移
    expect(screen.getByLabelText("上移任务完成率")).toBeDisabled();
    expect(screen.getByLabelText("下移大型任务完成率")).toBeDisabled();
  });
});
