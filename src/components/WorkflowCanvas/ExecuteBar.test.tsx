import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ExecuteBar } from "./ExecuteBar";

afterEach(() => cleanup());

const base = {
  doneCount: 0,
  total: 5,
  running: false,
  dirty: false,
  hasActive: true,
  onStart: () => {},
  onStop: () => {},
};

describe("ExecuteBar 状态与文案", () => {
  it("无任务卡 → empty 且禁用", () => {
    render(<ExecuteBar {...base} total={0} />);
    const bar = screen.getByTestId("exec-bar");
    expect(bar).toHaveAttribute("data-phase", "empty");
    expect(bar).toBeDisabled();
    expect(bar.textContent).toContain("请先添加任务卡");
  });

  it("未选工作流 → blocked 且禁用", () => {
    render(<ExecuteBar {...base} hasActive={false} />);
    const bar = screen.getByTestId("exec-bar");
    expect(bar).toHaveAttribute("data-phase", "blocked");
    expect(bar).toBeDisabled();
  });

  it("有未保存改动 → dirty 且禁用", () => {
    render(<ExecuteBar {...base} dirty />);
    const bar = screen.getByTestId("exec-bar");
    expect(bar).toHaveAttribute("data-phase", "dirty");
    expect(bar).toBeDisabled();
    expect(bar.textContent).toContain("请先保存");
  });

  it("全部完成 → full 且禁用，显示已完成、液面 100%", () => {
    render(<ExecuteBar {...base} doneCount={5} />);
    const bar = screen.getByTestId("exec-bar");
    expect(bar).toHaveAttribute("data-phase", "full");
    expect(bar).toBeDisabled();
    expect(bar.textContent).toContain("已完成");
    expect(bar).toHaveAttribute("data-pct", "100.00");
  });

  it("执行中 → running，点击调 onStop", () => {
    const onStop = vi.fn();
    render(<ExecuteBar {...base} running onStop={onStop} doneCount={2} />);
    const bar = screen.getByTestId("exec-bar");
    expect(bar).toHaveAttribute("data-phase", "running");
    expect(bar.textContent).toContain("执行中");
    expect(bar).not.toBeDisabled();
    fireEvent.click(bar);
    expect(onStop).toHaveBeenCalledTimes(1);
  });

  it("doneCount>0 未满 → continue，点击调 onStart", () => {
    const onStart = vi.fn();
    render(<ExecuteBar {...base} doneCount={2} onStart={onStart} />);
    const bar = screen.getByTestId("exec-bar");
    expect(bar).toHaveAttribute("data-phase", "continue");
    expect(bar.textContent).toContain("继续执行");
    fireEvent.click(bar);
    expect(onStart).toHaveBeenCalledTimes(1);
  });

  it("doneCount=0 → ready，点击调 onStart", () => {
    const onStart = vi.fn();
    render(<ExecuteBar {...base} onStart={onStart} />);
    const bar = screen.getByTestId("exec-bar");
    expect(bar).toHaveAttribute("data-phase", "ready");
    expect(bar.textContent).toContain("开始执行");
    fireEvent.click(bar);
    expect(onStart).toHaveBeenCalledTimes(1);
  });

  it("data-pct 反映 doneCount/total", () => {
    render(<ExecuteBar {...base} doneCount={2} total={8} />);
    expect(screen.getByTestId("exec-bar")).toHaveAttribute("data-pct", "25.00");
  });

  it("液面宽度 style 跟随比例", () => {
    const { container } = render(
      <ExecuteBar {...base} doneCount={3} total={6} />,
    );
    const liquid = container.querySelector(".exec-bar__liquid") as HTMLElement;
    expect(liquid.style.width).toBe("50%");
  });

  it("禁用（dirty）时点击不调 onStart", () => {
    const onStart = vi.fn();
    render(<ExecuteBar {...base} dirty onStart={onStart} />);
    fireEvent.click(screen.getByTestId("exec-bar"));
    expect(onStart).not.toHaveBeenCalled();
  });

  it("doneCount 超 total 时 pct 夹到 100", () => {
    render(<ExecuteBar {...base} doneCount={9} total={5} />);
    expect(screen.getByTestId("exec-bar")).toHaveAttribute(
      "data-pct",
      "100.00",
    );
  });
});
