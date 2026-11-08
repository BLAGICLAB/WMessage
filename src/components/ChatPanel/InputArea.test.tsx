// 输入卡底栏的渲染契约：思考模式选项文案、发送/停止键在卡内、
// 模型下拉 chip 作为底栏唯一收缩项的防溢出机制、
// 窄宽度三档降级（compact 图标化 / mini 收进 ⋯ 菜单）。

import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { InputArea } from "./InputArea";
import { tierOf } from "./useComposerTier";

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

describe("tierOf 分档阈值", () => {
  it("量到 0（容器隐藏）回落 full；470 / 620 两条线切三档", () => {
    expect(tierOf(0)).toBe("full");
    expect(tierOf(469)).toBe("mini");
    expect(tierOf(470)).toBe("compact");
    expect(tierOf(619)).toBe("compact");
    expect(tierOf(620)).toBe("full");
  });
});

// 降级由 ResizeObserver 驱动而 jsdom 没有：装可控替身，测试直接发射卡片宽。
// 不装替身时 hook 回落 full——上面那些用例跑的正是这条路径。
type WidthEntry = { contentRect: { width: number } };
let cardResizeCallbacks: Array<(entries: WidthEntry[]) => void> = [];
class FakeCardResizeObserver {
  constructor(cb: (entries: WidthEntry[]) => void) {
    cardResizeCallbacks.push(cb);
  }
  observe() {}
  disconnect() {}
}

function fireCardWidth(width: number) {
  act(() => {
    cardResizeCallbacks.forEach((cb) => cb([{ contentRect: { width } }]));
  });
}

const EFFORT_TITLE = "推理强度：只影响当前会话的发送，不写入设置";

describe("InputArea 底栏窄宽度三档降级", () => {
  beforeEach(() => {
    cardResizeCallbacks = [];
    (globalThis as unknown as { ResizeObserver?: unknown }).ResizeObserver =
      FakeCardResizeObserver;
  });
  afterEach(() => {
    delete (globalThis as unknown as { ResizeObserver?: unknown })
      .ResizeObserver;
  });

  it("full 档：控件全文案，没有 ⋯ 钮", () => {
    renderInput();
    fireCardWidth(700);
    expect(screen.getByTitle(/执行过程详细度/).textContent).toContain("简洁");
    expect(screen.getByTitle(EFFORT_TITLE).textContent).toContain("默认");
    expect(screen.getByTitle(/授权模式 弹授权/).textContent).toContain("弹授权");
    expect(screen.queryByTitle("更多输入选项")).toBeNull();
  });

  it("compact 档：详细度与推理强度只留图标，现值退到 aria-label；⋯ 还不出现", () => {
    renderInput({
      effort: { base: "medium", effective: "high", isOverridden: true, menuOpen: false, setMenuOpen: vi.fn(), setForSession: vi.fn() },
    });
    fireCardWidth(550);
    const verbose = screen.getByTitle(/执行过程详细度/);
    expect(verbose.textContent).not.toContain("简洁");
    expect(verbose.getAttribute("aria-label")).toContain("简洁");
    const effortChip = screen.getByTitle(EFFORT_TITLE);
    expect(effortChip.textContent).not.toContain("高");
    expect(effortChip.getAttribute("aria-label")).toContain("高");
    // 会话覆盖信号不能随文案一起丢：图标旁留一枚圆点
    expect(effortChip.querySelector("span.rounded-full")).not.toBeNull();
    // 授权模式在 compact 档仍保留文案
    expect(screen.getByTitle(/授权模式 弹授权/).textContent).toContain("弹授权");
    expect(screen.queryByTitle("更多输入选项")).toBeNull();
  });

  it("compact 档未覆盖推理强度时不挂圆点", () => {
    renderInput();
    fireCardWidth(550);
    const effortChip = screen.getByTitle(EFFORT_TITLE);
    expect(effortChip.querySelector("span.rounded-full")).toBeNull();
  });

  it("mini 档：详细度与推理强度撤出主行，⋯ 钮出现，授权模式图标化", () => {
    renderInput();
    fireCardWidth(400);
    expect(screen.queryByTitle(/执行过程详细度/)).toBeNull();
    expect(screen.queryByTitle(EFFORT_TITLE)).toBeNull();
    expect(screen.getByTitle("更多输入选项")).not.toBeNull();
    const perm = screen.getByTitle(/授权模式 弹授权/);
    expect(perm.textContent).not.toContain("弹授权");
    expect(perm.getAttribute("aria-label")).toContain("弹授权");
    // 主行仍在：模型 chip 与发送键不随降级消失
    expect(screen.getByTitle("切换模型")).not.toBeNull();
    expect(screen.getByTitle("发送（Enter 发送，Shift+Enter 换行）")).not.toBeNull();
  });

  it("⋯ 菜单：详细度行点击循环一档，菜单保持开着", () => {
    const setLevel = vi.fn();
    renderInput({ verbose: { level: "concise", setLevel } });
    fireCardWidth(400);
    fireEvent.click(screen.getByTitle("更多输入选项"));
    fireEvent.click(screen.getByRole("menuitem", { name: /执行过程详细度/ }));
    expect(setLevel).toHaveBeenCalledWith("detailed");
    expect(screen.getByRole("menu")).not.toBeNull();
  });

  it("⋯ 菜单：推理强度选档走 setForSession，选完收起菜单", () => {
    const setForSession = vi.fn();
    renderInput({
      effort: { base: "medium", effective: "medium", isOverridden: false, menuOpen: false, setMenuOpen: vi.fn(), setForSession },
    });
    fireCardWidth(400);
    fireEvent.click(screen.getByTitle("更多输入选项"));
    fireEvent.click(screen.getByRole("menuitem", { name: "高" }));
    expect(setForSession).toHaveBeenCalledWith("high");
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("⋯ 菜单：选「默认」清掉会话覆盖", () => {
    const setForSession = vi.fn();
    renderInput({
      effort: { base: "medium", effective: "high", isOverridden: true, menuOpen: false, setMenuOpen: vi.fn(), setForSession },
    });
    fireCardWidth(400);
    fireEvent.click(screen.getByTitle("更多输入选项"));
    fireEvent.click(screen.getByRole("menuitem", { name: "默认" }));
    expect(setForSession).toHaveBeenCalledWith(null);
  });

  it("⋯ 菜单：点菜单外收起", () => {
    renderInput();
    fireCardWidth(400);
    fireEvent.click(screen.getByTitle("更多输入选项"));
    expect(screen.getByRole("menu")).not.toBeNull();
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("拖窗跨回宽档：开着的 ⋯ 菜单随按钮一起收掉，再回窄档不会凭空开着", () => {
    renderInput();
    fireCardWidth(400);
    fireEvent.click(screen.getByTitle("更多输入选项"));
    expect(screen.getByRole("menu")).not.toBeNull();
    fireCardWidth(700);
    expect(screen.queryByRole("menu")).toBeNull();
    fireCardWidth(400);
    expect(screen.queryByRole("menu")).toBeNull();
  });
});
