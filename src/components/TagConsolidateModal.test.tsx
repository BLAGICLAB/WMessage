import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TagConsolidateModal } from "./TagConsolidateModal";
import { tagsConsolidateMerge, tagsConsolidateScan } from "../lib/botConsolidate";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
  convertFileSrc: vi.fn((p: string) => `tauri://localhost/${p}`),
}));

vi.mock("../lib/botConsolidate", async () => {
  const actual = await vi.importActual<typeof import("../lib/botConsolidate")>(
    "../lib/botConsolidate",
  );
  return {
    ...actual,
    tagsConsolidateScan: vi.fn(),
    tagsConsolidateMerge: vi.fn(async () => ({
      run_id: "tgc-1",
      merged_to: "x",
      merged_from: [],
      affected_task_ids: [],
      previous_tags: {},
      timestamp_ms: 0,
    })),
  };
});

const cluster = {
  members: ["买菜", "购菜"],
  suggested_main: "买菜",
  avg_similarity: 0.95,
  affected_task_count: 2,
};

beforeEach(() => {
  vi.mocked(tagsConsolidateScan).mockResolvedValue({
    scanned_tags: 2,
    unique_tags: 2,
    archived_tasks_scanned: 2,
    clusters: [cluster],
  });
  vi.mocked(tagsConsolidateMerge).mockClear();
});

describe("标签整理弹窗：新建主标签", () => {
  it("选「+ 新建主标签…」后输入框能一直接着打字（不被第一个字符顶掉）", async () => {
    const user = userEvent.setup();
    render(<TagConsolidateModal onClose={() => {}} />);
    await screen.findByText(/个待合并簇/);

    await user.selectOptions(
      screen.getByRole("combobox"),
      "__new__",
    );
    const input = screen.getByLabelText("新主标签名");
    await user.type(input, "买菜");
    expect(
      (input as HTMLInputElement).value,
      "输入框必须保留完整输入",
    ).toBe("买菜");
    expect(screen.getByLabelText("新主标签名")).toBeTruthy();
  });

  it("没填新主标签名时提交按钮禁用，且不会发出合并请求", async () => {
    const user = userEvent.setup();
    render(<TagConsolidateModal onClose={() => {}} />);
    await screen.findByText(/个待合并簇/);

    await user.selectOptions(screen.getByRole("combobox"), "__new__");
    const submit = screen.getByRole("button", { name: /合并 1 个簇/ });
    expect((submit as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("请填写新主标签名")).toBeTruthy();

    await user.click(submit);
    expect(tagsConsolidateMerge).not.toHaveBeenCalled();
  });

  it("填了合法名字才放行：主标签用新名字，别名按簇成员扣掉主标签本身", async () => {
    const user = userEvent.setup();
    render(<TagConsolidateModal onClose={() => {}} />);
    await screen.findByText(/个待合并簇/);

    await user.selectOptions(screen.getByRole("combobox"), "__new__");
    await user.type(screen.getByLabelText("新主标签名"), "  买菜  ");
    const submit = screen.getByRole("button", { name: /合并 1 个簇/ });
    await waitFor(() =>
      expect((submit as HTMLButtonElement).disabled).toBe(false),
    );

    await user.click(submit);
    await waitFor(() => expect(tagsConsolidateMerge).toHaveBeenCalledTimes(1));
    expect(tagsConsolidateMerge).toHaveBeenCalledWith("买菜", ["购菜"]);
  });

  it("新主标签名超长时拦下并提示（上限与后端一致）", async () => {
    const user = userEvent.setup();
    render(<TagConsolidateModal onClose={() => {}} />);
    await screen.findByText(/个待合并簇/);

    await user.selectOptions(screen.getByRole("combobox"), "__new__");
    await user.type(screen.getByLabelText("新主标签名"), "x".repeat(31));
    const submit = screen.getByRole("button", { name: /合并 1 个簇/ });
    expect((submit as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/新主标签名过长/)).toBeTruthy();
  });
});
