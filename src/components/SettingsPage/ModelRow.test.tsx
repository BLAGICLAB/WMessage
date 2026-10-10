// ModelRow 测试：连接测试（插头）与 onTested 回调契约。
// 回调语义：invoke 正常返回（含 r.ok=false 受控失败，后端已落盘 verified_vendors）
// 才通知父级刷新；invoke 异常（命令失败，后端没写名单）不回调。

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));

import { ModelRow } from "./ModelRow";
import type { ModelEntry } from "./types";

const baseModel: ModelEntry = {
  id: "m1",
  label: "DeepSeek Chat",
  baseUrl: "https://api.deepseek.com/v1",
  model: "deepseek-chat",
};

function renderRow(onTested?: () => void) {
  return render(
    <ModelRow
      model={baseModel}
      apiProvider="openai"
      onChange={() => {}}
      onDelete={() => {}}
      onTested={onTested}
    />,
  );
}

const plugBtn = () => screen.getByRole("button", { name: "测试连接" });

beforeEach(() => {
  mocks.invokeMock.mockReset();
});

describe("ModelRow 连接测试与 onTested 契约", () => {
  it("测试成功：行内转绿 + bot_test_connection 带厂商参数 + onTested 回调一次", async () => {
    mocks.invokeMock.mockResolvedValue({ ok: true, status: 200 });
    const onTested = vi.fn();
    renderRow(onTested);
    const user = userEvent.setup();
    await user.click(plugBtn());
    await waitFor(() =>
      expect(plugBtn().className).toContain("text-[var(--success)]"),
    );
    expect(mocks.invokeMock).toHaveBeenCalledWith("bot_test_connection", {
      baseUrl: baseModel.baseUrl,
      apiFormat: "openai",
      vendor: null,
    });
    expect(onTested).toHaveBeenCalledTimes(1);
  });

  it("受控失败（r.ok=false）：行内标红且 onTested 仍回调（后端已把厂商移出名单）", async () => {
    mocks.invokeMock.mockResolvedValue({ ok: false, error: "HTTP 401" });
    const onTested = vi.fn();
    renderRow(onTested);
    const user = userEvent.setup();
    await user.click(plugBtn());
    expect(await screen.findByTitle(/HTTP 401/)).toBeInTheDocument();
    expect(plugBtn().className).toContain("text-[var(--danger)]");
    expect(onTested).toHaveBeenCalledTimes(1);
  });

  it("invoke 异常：行内标红但 onTested 不回调（后端没写 verified_vendors，父级无需刷新）", async () => {
    mocks.invokeMock.mockRejectedValue({ code: "X", message: "后端命令失败" });
    const onTested = vi.fn();
    renderRow(onTested);
    const user = userEvent.setup();
    await user.click(plugBtn());
    expect(await screen.findByTitle(/后端命令失败/)).toBeInTheDocument();
    expect(plugBtn().className).toContain("text-[var(--danger)]");
    // 留出微任务余量后仍不得回调
    await act(async () => {});
    expect(onTested).not.toHaveBeenCalled();
  });
});
