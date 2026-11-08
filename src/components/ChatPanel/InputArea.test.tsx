// 输入卡底栏的渲染契约：思考模式选项文案、发送/停止键在卡内、
// 放不下时逐级图标化（推理强度 → 授权 → 模型，单行不换行、无 ⋯ 菜单）。

import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
// 文字态才成立——图标态整枚钉死宽度（chatLayout 推导链保证放得下）。
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

// 定档由 ResizeObserver + canvas 测字驱动，jsdom 两样都没有：装可控替身——
// 发射卡片宽，canvas 量宽按「字符数 × 12」假。不装替身时 hook 回落 0 档
// （全带文字），上面那些用例跑的正是这条路径。
type WidthEntry = { contentRect: { width: number } };
let cardResizeCallbacks: Array<(entries: WidthEntry[]) => void> = [];
class FakeCardResizeObserver {
  constructor(cb: (entries: WidthEntry[]) => void) {
    cardResizeCallbacks.push(cb);
  }
  observe() {}
  disconnect() {}
}

const realGetContext = HTMLCanvasElement.prototype.getContext;
function installFakeCanvas() {
  HTMLCanvasElement.prototype.getContext = function fakeGetContext() {
    return {
      measureText: (t: string) => ({ width: t.length * 12 }),
      set font(_v: string) {},
    } as unknown as CanvasRenderingContext2D;
  } as unknown as typeof HTMLCanvasElement.prototype.getContext;
}

function fireCardWidth(width: number) {
  act(() => {
    cardResizeCallbacks.forEach((cb) => cb([{ contentRect: { width } }]));
  });
}

const EFFORT_TITLE = "推理强度：只影响当前会话的发送，不写入设置";
const MODEL_ICON_TITLE = "切换模型（当前：MiniMax-M3.1-Flash）";

describe("InputArea 底栏逐级图标化", () => {
  beforeEach(() => {
    cardResizeCallbacks = [];
    (globalThis as unknown as { ResizeObserver?: unknown }).ResizeObserver =
      FakeCardResizeObserver;
    installFakeCanvas();
  });
  afterEach(() => {
    HTMLCanvasElement.prototype.getContext = realGetContext;
    delete (globalThis as unknown as { ResizeObserver?: unknown })
      .ResizeObserver;
  });

  it("宽裕（500）：三枚控件全带文字，没有 ⋯ 钮", () => {
    renderInput();
    fireCardWidth(500);
    expect(screen.getByTitle(EFFORT_TITLE).textContent).toContain("默认");
    expect(screen.getByTitle(/授权模式 弹授权/).textContent).toContain("弹授权");
    expect(screen.getByTitle("切换模型").textContent).toContain("MiniMax");
    expect(screen.queryByTitle("更多输入选项")).toBeNull();
  });

  it("第 1 档（400）：只有推理强度变图标，授权与模型仍带文字", () => {
    renderInput();
    fireCardWidth(400);
    const effort = screen.getByTitle(EFFORT_TITLE);
    expect(effort.textContent).not.toContain("默认");
    expect(effort.getAttribute("aria-label")).toContain("默认");
    expect(screen.getByTitle(/授权模式 弹授权/).textContent).toContain("弹授权");
    expect(screen.getByTitle("切换模型").textContent).toContain("MiniMax");
    expect(screen.queryByTitle("更多输入选项")).toBeNull();
  });

  it("第 2 档（370）：推理强度与授权图标化，模型仍带文字", () => {
    renderInput();
    fireCardWidth(370);
    const perm = screen.getByTitle(/授权模式 弹授权/);
    expect(perm.textContent).not.toContain("弹授权");
    expect(perm.getAttribute("aria-label")).toContain("弹授权");
    expect(screen.getByTitle("切换模型").textContent).toContain("MiniMax");
  });

  it("第 3 档（300）：全图标一行，宽度钉死在推导链常量上", () => {
    renderInput();
    fireCardWidth(300);
    const effort = screen.getByTitle(EFFORT_TITLE);
    const perm = screen.getByTitle(/授权模式 弹授权/);
    const model = screen.getByTitle(MODEL_ICON_TITLE);
    // 图标态补 aria-label（可见文字没了）
    expect(effort.getAttribute("aria-label")).toContain("默认");
    expect(perm.getAttribute("aria-label")).toContain("弹授权");
    expect(model.getAttribute("aria-label")).toContain("MiniMax");
    // 图标态宽度钉死：三枚下拉 40、＋ 28、发送 32，间距 4——加起来即
    // 全图标行宽 200，也是中栏最小宽度的依据
    expect(effort.style.width).toBe("40px");
    expect(perm.style.width).toBe("40px");
    expect(model.style.width).toBe("40px");
    expect(screen.getByLabelText("添加文件或图片").style.width).toBe("28px");
    expect(
      (screen.getByTitle("发送（Enter 发送，Shift+Enter 换行）") as HTMLElement)
        .style.width,
    ).toBe("32px");
    // 单行不换行
    expect(
      (screen.getByTitle(EFFORT_TITLE).parentElement?.parentElement ?? null)
        ?.className,
    ).not.toContain("flex-wrap");
  });

  it("换模型名重新定档：同一个宽度，短名全带文字、长名模型让位（第 2 档）", () => {
    const short = renderInput({
      model: {
        models: [],
        provider: "openai",
        activeId: null,
        label: "gpt-4o",
        menuOpen: false,
        setMenuOpen: vi.fn(),
        onSwitch: vi.fn(),
      },
    });
    fireCardWidth(380);
    // gpt-4o（6 字符 ×12=72）放得下：三枚全带文字
    expect(screen.getByTitle("切换模型").textContent).toContain("gpt-4o");
    short.unmount();
    renderInput();
    // MiniMax-M3.1-Flash 量宽 216、按 150 封顶：同宽度下先让推理强度与授权
    fireCardWidth(380);
    expect(screen.getByTitle(/授权模式 弹授权/).textContent).not.toContain(
      "弹授权",
    );
    expect(screen.getByTitle("切换模型").textContent).toContain("MiniMax");
  });

  it("会话覆盖推理强度：图标态 ⚡ 右上角挂角标；未覆盖不挂", () => {
    const withDot = renderInput({
      effort: { base: "medium", effective: "high", isOverridden: true, menuOpen: false, setMenuOpen: vi.fn(), setForSession: vi.fn() },
    });
    fireCardWidth(400);
    expect(screen.getByTestId("effort-override-dot")).not.toBeNull();
    withDot.unmount();
    renderInput();
    fireCardWidth(400);
    expect(screen.queryByTestId("effort-override-dot")).toBeNull();
  });

  it("详细度与 ⋯ 菜单均已退役：任何宽度档都不再出现", () => {
    renderInput();
    fireCardWidth(500);
    expect(screen.queryByTitle(/执行过程详细度/)).toBeNull();
    expect(screen.queryByTitle("更多输入选项")).toBeNull();
    fireCardWidth(300);
    expect(screen.queryByTitle(/执行过程详细度/)).toBeNull();
    expect(screen.queryByTitle("更多输入选项")).toBeNull();
  });
});
