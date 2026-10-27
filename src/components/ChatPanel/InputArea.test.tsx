// 输入卡底栏的渲染契约：思考模式选项文案、发送/停止键在卡内、
// 模型下拉 chip 作为底栏唯一收缩项的防溢出机制。

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { InputArea } from "./InputArea";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (p: string) => p,
}));

function renderInput(over: Partial<Parameters<typeof InputArea>[0]> = {}) {
  const base: Parameters<typeof InputArea>[0] = {
    input: "",
    setInput: vi.fn(),
    files: [],
    setFiles: vi.fn(),
    taskRefs: [],
    setTaskRefs: vi.fn(),
    taRef: { current: null },
    slashIdx: 0,
    setSlashIdx: vi.fn(),
    slashDismissed: true,
    setSlashDismissed: vi.fn(),
    send: vi.fn(),
    pickFiles: vi.fn(),
    onStop: vi.fn(),
    viewedBusy: false,
    perm: {
      mode: "ask",
      menuOpen: false,
      setMenuOpen: vi.fn(),
      onSelect: vi.fn(),
    },
    permBtnRef: { current: null },
    permDropdownRef: { current: null },
    verbose: { level: "concise", setLevel: vi.fn() },
    modelBtnRef: { current: null },
    modelDropdownRef: { current: null },
    effortBtnRef: { current: null },
    effortDropdownRef: { current: null },
    model: {
      models: [],
      provider: "openai",
      activeId: null,
      label: "MiniMax-M3.1-Flash",
      menuOpen: false,
      setMenuOpen: vi.fn(),
      onSwitch: vi.fn(),
    },
    effort: {
      base: "medium",
      effective: "medium",
      isOverridden: false,
      menuOpen: false,
      setMenuOpen: vi.fn(),
      setForSession: vi.fn(),
    },
  };
  return render(<InputArea {...base} {...over} />);
}

describe("InputArea 思考模式下拉", () => {
  it("首项是「默认」，不再复读后台档位；悬停才解释默认跟的是哪档", () => {
    renderInput({ effort: { base: "medium", effective: "medium", isOverridden: false, menuOpen: true, setMenuOpen: vi.fn(), setForSession: vi.fn() } });
    const first = screen.getByTitle(/清掉本会话覆盖/);
    expect(first.textContent).toBe("默认");
    expect(screen.queryByText(/后台默认（/)).toBeNull();
  });

  it("未覆盖时按钮就叫「默认」，不再拼成「中·默认」", () => {
    renderInput({ effort: { base: "medium", effective: "medium", isOverridden: false, menuOpen: false, setMenuOpen: vi.fn(), setForSession: vi.fn() } });
    const chip = screen.getByTitle("推理强度：只影响当前会话的发送，不写入设置");
    expect(chip.textContent).toContain("默认");
    expect(chip.textContent).not.toContain("·");
  });

  it("覆盖后按钮显示所选档位，不再缀「默认」", () => {
    renderInput({ effort: { base: "medium", effective: "high", isOverridden: true, menuOpen: false, setMenuOpen: vi.fn(), setForSession: vi.fn() } });
    const chip = screen.getByTitle("推理强度：只影响当前会话的发送，不写入设置");
    expect(chip.textContent).toContain("高");
    expect(chip.textContent).not.toContain("默认");
  });
});

describe("InputArea 发送/停止键在输入卡内", () => {
  it("空闲时发送键在输入卡内", () => {
    renderInput();
    const send = screen.getByTitle("发送（Enter 发送，Shift+Enter 换行）");
    expect(send.closest(".nm-card")).not.toBeNull();
  });

  it("回复中停止键同样在输入卡内", () => {
    renderInput({ viewedBusy: true });
    const stop = screen.getByTitle("停止当前回复");
    expect(stop.closest(".nm-card")).not.toBeNull();
  });
});

// 底栏其余芯片全是 shrink-0：空间不足时只有模型 chip 收缩（标签多截一点），
// 它若也被钉死，发送/停止键就被顶出输入卡右缘（实机窄窗踩过）。
describe("InputArea 模型下拉 chip 是底栏泄压阀", () => {
  it("chip 可收缩（min-w-0、无 shrink-0），标签宽度上限仍是 150px", () => {
    renderInput();
    const chip = screen.getByTitle("切换模型");
    expect(chip.className).toContain("min-w-0");
    const wrap = chip.parentElement as HTMLElement;
    expect(wrap.className).toContain("min-w-0");
    expect(wrap.className).not.toContain("shrink-0");
    expect(chip.querySelector('span[class*="max-w-[150px]"]')).not.toBeNull();
  });
});
