import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { RichText } from "./RichText";

const mocks = vi.hoisted(() => ({
  openTargetMock: vi.fn(),
}));

vi.mock("../../lib/openTarget", async (importActual) => {
  const actual = await importActual<typeof import("../../lib/openTarget")>();
  return { ...actual, openTarget: mocks.openTargetMock };
});

beforeEach(() => {
  mocks.openTargetMock.mockClear();
});

describe("RichText 链接可达性", () => {
  it("URL token：真 href + 点击调 openTarget（尾随标点剥离）", async () => {
    const user = userEvent.setup();
    render(<RichText text={"详见 https://example.com/a, 末尾"} />);
    const link = screen.getByRole("link", { name: "https://example.com/a" });
    expect(link).toHaveAttribute("href", "https://example.com/a");
    await user.click(link);
    expect(mocks.openTargetMock).toHaveBeenCalledWith("https://example.com/a");
  });

  it("路径 token：无 href（防 bogus URL 解析）+ role=link 可聚焦 + Enter 触发", async () => {
    const user = userEvent.setup();
    render(<RichText text={"打开 /Users/x/report.pdf 看看"} />);
    const link = screen.getByRole("link", { name: "/Users/x/report.pdf" });
    expect(link).not.toHaveAttribute("href");
    expect(link).toHaveAttribute("tabindex", "0");
    link.focus();
    await user.keyboard("{Enter}");
    expect(mocks.openTargetMock).toHaveBeenCalledWith("/Users/x/report.pdf");
  });
});
