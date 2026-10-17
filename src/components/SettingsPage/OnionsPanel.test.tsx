import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import { OnionsPanel } from "./OnionsPanel";
import type { OnionSummary } from "./types";

const ok = (n: string, over: Partial<OnionSummary> = {}): OnionSummary => ({
  name: n,
  role: `角色-${n}`,
  profile: "coder",
  allowedCount: 5,
  maxTurn: 4,
  broken: null,
  builtin: false,
  ...over,
});

// 返回类型显式标出：默认实现若被推断成窄类型（如 Promise<never[]>），
// 后面的 mockImplementation / mockResolvedValueOnce 都会因签名不符而报错，
// 也会让 calls[1] 取参数时的元组长度对不上。
const mocks = vi.hoisted(() => ({
  invokeMock: vi.fn(
    async (cmd: string, _args?: unknown): Promise<unknown> => {
      if (cmd === "onions_list") return [];
      if (cmd === "onions_import") return "mine";
      if (cmd === "onions_export") return "/tmp/out.yaml";
      return null;
    },
  ),
  openMock: vi.fn(async (): Promise<string | null> => "/tmp/mine.yaml"),
  saveMock: vi.fn(async (): Promise<string | null> => "/tmp/out.yaml"),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.openMock,
  save: mocks.saveMock,
}));

beforeEach(() => {
  mocks.invokeMock.mockClear();
  mocks.openMock.mockClear();
  mocks.saveMock.mockClear();
});

async function flush() {
  await act(async () => {});
}

/** 让下一次 onions_list 返回给定列表 */
function listReturns(rows: OnionSummary[]) {
  mocks.invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "onions_list") return rows;
    if (cmd === "onions_import") return "mine";
    if (cmd === "onions_export") return "/tmp/out.yaml";
    return null;
  });
}

describe("OnionsPanel 列表", () => {
  it("空列表给出可操作的空状态，而不是空白", async () => {
    render(<OnionsPanel />);
    await flush();
    expect(screen.getByText("暂无配置档")).toBeInTheDocument();
    expect(screen.getByText(/导入配置档/)).toBeInTheDocument();
  });

  it("坏文件必须显形并说明坏在哪（用户看不见就永远修不好）", async () => {
    listReturns([
      ok("probe", { builtin: true }),
      ok("mine", { broken: "allowed 里的「x」不在内置工具表 TOOLS_TABLE 里" }),
    ]);
    render(<OnionsPanel />);
    await flush();
    expect(screen.getByText("probe")).toBeInTheDocument();
    // 内置标记要能看出来（用户要区分「改坏了内置」与「自己导入的」）
    expect(screen.getByText("（内置）")).toBeInTheDocument();
    expect(screen.getByText(/此文件无效/)).toBeInTheDocument();
    expect(screen.getByText(/TOOLS_TABLE/)).toBeInTheDocument();
  });

  it("列表命令返回异常值时按空列表渲染，不得白屏", async () => {
    // 后端命令改名/返回异常时，列表空着远好过整个设置页崩掉——设置页崩了
    // 用户连改设置都改不了。这条会逼着 refresh 里的兜底保留。
    mocks.invokeMock.mockImplementation(async () => null);
    render(<OnionsPanel />);
    await flush();
    expect(screen.getByText("暂无配置档")).toBeInTheDocument();
  });

  it("健康配置展示档位 / 工具数 / 轮数上限", async () => {
    listReturns([ok("validator", { profile: "coder", allowedCount: 7, maxTurn: 4 })]);
    render(<OnionsPanel />);
    await flush();
    expect(screen.getByText(/coder/)).toBeInTheDocument();
    expect(screen.getByText(/工具 7 个/)).toBeInTheDocument();
    expect(screen.getByText(/轮数上限 4/)).toBeInTheDocument();
  });
});

describe("OnionsPanel 导入导出", () => {
  it("导入把选中路径传给后端，并刷新列表", async () => {
    let listed = false;
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "onions_list") {
        listed = true;
        return listed ? [ok("mine")] : [];
      }
      if (cmd === "onions_import") return "mine";
      return null;
    });
    render(<OnionsPanel />);
    await flush();
    fireEvent.click(screen.getByText(/导入配置档/));
    await flush();
    expect(mocks.openMock).toHaveBeenCalled();
    const importCall = mocks.invokeMock.mock.calls.find((c) => c[0] === "onions_import");
    expect(importCall?.[1]).toEqual({ path: "/tmp/mine.yaml" });
    expect(await screen.findByText("mine")).toBeInTheDocument();
  });

  it("用户取消选择对话框时不得发导入命令", async () => {
    mocks.openMock.mockResolvedValueOnce(null);
    render(<OnionsPanel />);
    await flush();
    fireEvent.click(screen.getByText(/导入配置档/));
    await flush();
    expect(
      mocks.invokeMock.mock.calls.some((c) => c[0] === "onions_import"),
    ).toBe(false);
  });

  it("导入失败：错误必须显示出来，不能静默", async () => {
    mocks.invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "onions_list") return [];
      if (cmd === "onions_import") throw new Error("配置档「probe」已存在：不会静默覆盖");
      return null;
    });
    render(<OnionsPanel />);
    await flush();
    fireEvent.click(screen.getByText(/导入配置档/));
    await flush();
    expect(screen.getByText(/不会静默覆盖/)).toBeInTheDocument();
  });

  it("导出：把 name 与目标路径都传给后端", async () => {
    listReturns([ok("probe", { builtin: true })]);
    render(<OnionsPanel />);
    await flush();
    fireEvent.click(screen.getByLabelText("导出配置档 probe"));
    await flush();
    expect(mocks.saveMock).toHaveBeenCalled();
    const exportCall = mocks.invokeMock.mock.calls.find((c) => c[0] === "onions_export");
    expect(exportCall?.[1]).toEqual({ name: "probe", path: "/tmp/out.yaml" });
  });

  it("坏文件也要能导出——坏在字段上，原文正是要手改的东西", async () => {
    listReturns([ok("mine", { broken: "YAML 解析失败" })]);
    render(<OnionsPanel />);
    await flush();
    fireEvent.click(screen.getByLabelText("导出配置档 mine"));
    await flush();
    expect(
      mocks.invokeMock.mock.calls.some((c) => c[0] === "onions_export"),
    ).toBe(true);
  });

  it("导出对话框取消时不发命令", async () => {
    listReturns([ok("probe")]);
    mocks.saveMock.mockResolvedValueOnce(null);
    render(<OnionsPanel />);
    await flush();
    fireEvent.click(screen.getByLabelText("导出配置档 probe"));
    await flush();
    expect(
      mocks.invokeMock.mock.calls.some((c) => c[0] === "onions_export"),
    ).toBe(false);
  });
});
