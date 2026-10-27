import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MessageList } from "./MessageList";
import type { Msg } from "./types";

// 交付物区要问后端「这个路径在磁盘上真在吗」——这里把 IPC 打桩成可控的
// 存在性判定，测的是前端口径（谁该挂、谁该挡、谁该被剔）。
const mocks = vi.hoisted(() => ({
  invokeMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invokeMock,
}));

vi.mock("../ActorAvatar", () => ({
  ActorAvatar: ({ bot }: { bot: boolean }) => (
    <span data-testid="avatar" data-bot={String(bot)} />
  ),
}));

const baseProps = {
  scrollRef: { current: null },
  viewedBusy: false,
  copiedIdx: null,
  verboseLevel: "detailed" as const,
  onCopy: () => {},
  onRemove: () => {},
  onOpenTask: () => {},
  onOpenExecSession: () => {},
  onPickStarter: () => {},
};

/** 打桩存在性判定：清单里列出的路径才算存在 */
function stubProbe(existing: string[]) {
  mocks.invokeMock.mockImplementation(async (cmd: string, args?: { paths?: string[] }) => {
    if (cmd === "bot_probe_paths") {
      return (args?.paths ?? []).map((p) => ({
        path: p,
        exists: existing.includes(p),
        isDir: false,
        size: 10,
      }));
    }
    return null;
  });
}

beforeEach(() => {
  mocks.invokeMock.mockReset();
});

/**
 * 交付物区：只放最终产物。
 *
 * 之前那个位置摆的是「执行过程文件」——一轮 PPT 生成里 11 个 .py 脚本把真正
 * 要拿的 .pptx 挤没了。交付物位的口径是**最终产物**：正文自报的路径
 * （模型点名了什么就是什么）∪ 本轮写盘产生的文件（过形态过滤，脚本挡掉），
 * 再按磁盘存在性收敛。
 */
describe("气泡交付物区", () => {
  it("正文点名的路径 + 写盘产生的 pptx 都挂上；.py 脚本一个都不进", async () => {
    stubProbe(["/Users/x/宣贯.pptx", "/Users/x/设计说明.md"]);
    const messages: Msg[] = [
      {
        role: "assistant",
        content: "已生成 /Users/x/宣贯.pptx 与 /Users/x/设计说明.md",
        fileChanges: [
          { path: "/Users/x/宣贯.pptx", kind: "create", added: 12, deleted: 0 },
          { path: "/Users/x/设计说明.md", kind: "create", added: 3, deleted: 0 },
          { path: "/Users/x/gen_slides.py", kind: "create", added: 462, deleted: 0 },
        ],
      },
    ];
    render(<MessageList messages={messages} {...baseProps} />);

    expect(
      await screen.findByTitle("打开交付物：/Users/x/宣贯.pptx"),
    ).toBeInTheDocument();
    expect(
      screen.getByTitle("打开交付物：/Users/x/设计说明.md"),
    ).toBeInTheDocument();
    // 脚本形态不是交付物——挂在交付物位上会把真正要拿的那份淹没
    expect(
      screen.queryByTitle(/gen_slides\.py/),
    ).toBeNull();
  });

  it("chip 只有图标 + 文件名，没有 ±行数（行数是轨迹语义，不是交付物语义）", async () => {
    stubProbe(["/Users/x/宣贯.pptx"]);
    const messages: Msg[] = [
      {
        role: "assistant",
        content: "已生成 /Users/x/宣贯.pptx",
        fileChanges: [
          { path: "/Users/x/宣贯.pptx", kind: "create", added: 462, deleted: 7 },
        ],
      },
    ];
    render(<MessageList messages={messages} {...baseProps} />);
    const chip = await screen.findByTitle("打开交付物：/Users/x/宣贯.pptx");
    expect(chip).toHaveTextContent("宣贯.pptx");
    expect(chip).not.toHaveTextContent("+462");
    expect(chip).not.toHaveTextContent("−7");
  });

  it("磁盘上不存在的路径不进交付物区（挂一个点不开的条目比不给更糟）", async () => {
    stubProbe([]);
    const messages: Msg[] = [
      { role: "assistant", content: "已生成 /Users/x/宣贯.pptx" },
    ];
    render(<MessageList messages={messages} {...baseProps} />);
    await waitFor(() => {
      expect(mocks.invokeMock).toHaveBeenCalledWith("bot_probe_paths", {
        paths: ["/Users/x/宣贯.pptx"],
      });
    });
    expect(screen.queryByTitle(/宣贯\.pptx/)).toBeNull();
  });

  it("点交付物：走 onPreviewFile（跳右侧栏预览），不调起系统默认程序", async () => {
    const user = userEvent.setup();
    stubProbe(["/Users/x/宣贯.pptx"]);
    const onPreviewFile = vi.fn();
    const messages: Msg[] = [
      { role: "assistant", content: "已生成 /Users/x/宣贯.pptx" },
    ];
    render(
      <MessageList
        messages={messages}
        {...baseProps}
        onPreviewFile={onPreviewFile}
      />,
    );
    await user.click(
      await screen.findByTitle("打开交付物：/Users/x/宣贯.pptx"),
    );
    expect(onPreviewFile).toHaveBeenCalledWith("/Users/x/宣贯.pptx");
  });

  it("这轮确实没产出：交付物区不留空行", async () => {
    stubProbe([]);
    const messages: Msg[] = [
      { role: "assistant", content: "好的，已经处理完了" },
    ];
    const { container } = render(
      <MessageList messages={messages} {...baseProps} />,
    );
    await waitFor(() => {
      expect(screen.queryByText(/未在回复中列出/)).toBeNull();
    });
    expect(container.textContent).not.toContain("交付物");
  });

  it("写了文件但模型没在正文点名：给一行提示并可跳去看改动", async () => {
    stubProbe(["/Users/x/gen_slides.py"]);
    const onShowFileChanges = vi.fn();
    const user = userEvent.setup();
    const messages: Msg[] = [
      {
        role: "assistant",
        content: "已经做好了",
        fileChanges: [
          { path: "/Users/x/gen_slides.py", kind: "create", added: 462, deleted: 0 },
        ],
      },
    ];
    render(
      <MessageList
        messages={messages}
        {...baseProps}
        onShowFileChanges={onShowFileChanges}
      />,
    );
    // 写出来的全是脚本形态：合并后交付物区是空的，但「有东西没给你」这件事
    // 得说清楚——用户该知道去哪找，而不是以为这轮什么都没产出
    const hint = await screen.findByText(/未在回复中列出/);
    expect(hint).toBeInTheDocument();
    await user.click(hint);
    expect(onShowFileChanges).toHaveBeenCalled();
  });

  it("写出来的文件本身是交付物形态：直接挂上（不靠正文点名）", async () => {
    stubProbe(["/Users/x/宣贯.pptx"]);
    const messages: Msg[] = [
      {
        role: "assistant",
        content: "已经做好了",
        fileChanges: [
          { path: "/Users/x/宣贯.pptx", kind: "create", added: 12, deleted: 0 },
        ],
      },
    ];
    render(<MessageList messages={messages} {...baseProps} />);
    // 新产生那半是兜底：模型没点名，但产物确实是交付物形态，就该挂上
    expect(
      await screen.findByTitle("打开交付物：/Users/x/宣贯.pptx"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/未在回复中列出/)).toBeNull();
  });
});
