// InstructionsCard（设置页「memory」分区 · 指令文件卡）单测。
//
// 盯住三件容易出事的：① 载入把盘上内容回填进输入框；② 保存走
// instructions_set 且带 content（漏传 = 后端拿到空，把指令清没了）；
// ③ 超限内容禁保存钮 + 显提示——闸在前端挡住，别让用户点完才见错。

import { it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { InstructionsCard } from "./InstructionsCard";

// vi.mock 工厂会被提升到文件顶部，invokeMock 必须走 vi.hoisted 同步提升
const mocks = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invokeMock }));

beforeEach(() => {
  mocks.invokeMock.mockReset();
  cleanup();
});

const view = (content: string) => ({
  content,
  chars: [...content].length,
  maxChars: 100,
});

const textarea = () =>
  screen.getByLabelText("指令文件内容") as HTMLTextAreaElement;

const saveButton = () =>
  screen.getByRole("button", { name: "保存" }) as HTMLButtonElement;

it("载入把盘上内容回填进输入框并显示字数", async () => {
  mocks.invokeMock.mockResolvedValue(view("永远用中文回复"));
  render(<InstructionsCard />);
  await waitFor(() => expect(textarea().value).toBe("永远用中文回复"));
  expect(screen.getByText("7/100 字")).toBeTruthy();
  expect(saveButton().disabled).toBe(true); // 未改动，保存钮禁用
});

it("保存走 instructions_set 且带上完整内容，成功后提示生效时机", async () => {
  mocks.invokeMock.mockResolvedValueOnce(view("永远用中文回复"));
  mocks.invokeMock.mockResolvedValueOnce(view("永远用中文回复。"));
  render(<InstructionsCard />);
  await waitFor(() => expect(textarea().value).toBe("永远用中文回复"));

  fireEvent.change(textarea(), { target: { value: "永远用中文回复。" } });
  expect(saveButton().disabled).toBe(false);
  fireEvent.click(saveButton());

  await waitFor(() => expect(screen.getByText("已保存，下一回合生效")).toBeTruthy());
  expect(mocks.invokeMock).toHaveBeenLastCalledWith("instructions_set", {
    content: "永远用中文回复。",
  });
});

it("超限内容禁用保存钮并显超限提示", async () => {
  mocks.invokeMock.mockResolvedValue(view("字".repeat(101)));
  render(<InstructionsCard />);
  await waitFor(() => expect(saveButton().disabled).toBe(true));
  expect(screen.getByText(/超出上限 101\/100 字/)).toBeTruthy();

  // 精简回上限内 → 保存钮恢复可用
  fireEvent.change(textarea(), { target: { value: "短了" } });
  expect(saveButton().disabled).toBe(false);
  expect(screen.getByText("2/100 字")).toBeTruthy();
});

it("保存失败把后端错误转成人话展示，不吞掉", async () => {
  mocks.invokeMock.mockResolvedValueOnce(view("abc"));
  mocks.invokeMock.mockRejectedValueOnce({ message: "落盘失败" });
  render(<InstructionsCard />);
  await waitFor(() => expect(textarea().value).toBe("abc"));

  // 先改动再保存——未改动时保存钮禁用，点击是空操作
  fireEvent.change(textarea(), { target: { value: "abcd" } });
  fireEvent.click(saveButton());
  await waitFor(() => expect(screen.getByText(/保存失败/)).toBeTruthy());
});
