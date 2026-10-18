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

// 流式中的路径与完成后的 markdown 路径是同一份文件，同一种点击
// （见 MarkdownText 同组用例）
describe("RichText 路径点击接管", () => {
  it("路径 token 交给 onPathClick，不调 openTarget", async () => {
    const user = userEvent.setup();
    const onPathClick = vi.fn();
    render(
      <RichText text={"产物 /Users/x/out.md 好了"} onPathClick={onPathClick} />,
    );
    const link = screen.getByRole("link", { name: "/Users/x/out.md" });
    expect(link).toHaveAttribute("title", "在右侧栏预览");
    await user.click(link);
    expect(onPathClick).toHaveBeenCalledWith("/Users/x/out.md");
    expect(mocks.openTargetMock).not.toHaveBeenCalled();
  });

  it("URL 仍走 openTarget，不被接管", async () => {
    const user = userEvent.setup();
    const onPathClick = vi.fn();
    render(
      <RichText text={"详见 https://example.com/a"} onPathClick={onPathClick} />,
    );
    await user.click(screen.getByRole("link", { name: "https://example.com/a" }));
    expect(mocks.openTargetMock).toHaveBeenCalledWith("https://example.com/a");
    expect(onPathClick).not.toHaveBeenCalled();
  });
});
