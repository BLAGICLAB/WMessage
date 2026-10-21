import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { NotificationsPage } from "./NotificationsPage";
import type { NotificationItem } from "../../lib/notifications";

// —— Tauri mocks（同 TrashPage.test 模式）——
const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}));

function mkItem(partial: Partial<NotificationItem>): NotificationItem {
  return {
    id: "n1",
    kind: "memory_proposal",
    title: "新记忆提案 · 1 条",
    body: "",
    payload: {},
    status: "pending",
    createdAt: 1760000000000,
    resolvedAt: null,
    ...partial,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
  // 默认：列表空（notifications_list）；其余命令返回成功
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "notifications_list") return [];
    if (cmd === "notifications_pending_count") return 0;
    return null;
  });
});

describe("NotificationsPage", () => {
  it("空列表：显示空状态文案", async () => {
    render(<NotificationsPage />);
    await waitFor(() =>
      expect(screen.getByText("没有待处理的通知")).toBeTruthy(),
    );
  });

  it("待处理页签只显示 pending 消息；已处理在「全部」页签置灰展示", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "notifications_list")
        return [
          mkItem({ id: "done1", status: "done", title: "已处理消息" }),
          mkItem({ id: "p1", title: "待处理消息" }),
        ];
      if (cmd === "notifications_pending_count") return 1;
      return null;
    });
    const { container } = render(<NotificationsPage />);
    await waitFor(() => expect(screen.getByText("待处理消息")).toBeTruthy());
    expect(screen.queryByText("已处理消息")).toBeNull();
    // 切「全部」页签
    await userEvent.click(screen.getByRole("tab", { name: "全部" }));
    expect(screen.getByText("已处理消息")).toBeTruthy();
    expect(screen.getByText("已处理")).toBeTruthy();
    expect(container).toBeTruthy();
  });

  it("记忆提案卡：全部收下 → mem_pending_approve(ids)", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "notifications_list")
        return [
          mkItem({
            id: "m1",
            kind: "memory_proposal",
            title: "新记忆提案 · 2 条",
            payload: { ids: [7, 8] },
          }),
        ];
      return null;
    });
    render(<NotificationsPage />);
    const btn = await screen.findByRole("button", { name: "全部收下（2）" });
    await userEvent.click(btn);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("mem_pending_approve", {
        ids: [7, 8],
      }),
    );
  });

  it("产物绑定卡：默认全选，绑定选中 → confirm_artifact_batch；跳过 → notifications_resolve(dismissed)", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "notifications_list")
        return [
          mkItem({
            id: "a1",
            kind: "artifact_bind",
            title: "任务「测试」完成，2 个产物待绑定",
            payload: { taskId: "t1", paths: ["/a/x.txt", "/a/y.txt"] },
          }),
        ];
      return null;
    });
    render(<NotificationsPage />);
    const bindBtn = await screen.findByRole("button", {
      name: "绑定选中（2）",
    });
    await userEvent.click(bindBtn);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("confirm_artifact_batch", {
        taskId: "t1",
        paths: ["/a/x.txt", "/a/y.txt"],
      }),
    );
  });

  it("产物绑定卡：跳过 → notifications_resolve(id, dismissed)", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "notifications_list")
        return [
          mkItem({
            id: "a2",
            kind: "artifact_bind",
            title: "任务「测试」完成，1 个产物待绑定",
            payload: { taskId: "t1", paths: ["/a/x.txt"] },
          }),
        ];
      return null;
    });
    render(<NotificationsPage />);
    const skipBtn = await screen.findByRole("button", { name: "跳过" });
    await userEvent.click(skipBtn);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("notifications_resolve", {
        id: "a2",
        status: "dismissed",
      }),
    );
  });

  it("服务告警卡：只告知不决策——「知道了」→ notifications_resolve(id, dismissed)", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "notifications_list")
        return [
          mkItem({
            id: "s1",
            kind: "service_alert",
            title: "前端 dev server · 端口违规",
            body: "监听地址 0.0.0.0 不是 loopback（L1）",
            payload: {
              serviceId: "svc_1",
              alertType: "port_violation",
              rule: "L1",
              address: "0.0.0.0",
              port: 7000,
            },
          }),
        ];
      return null;
    });
    render(<NotificationsPage />);
    // 标题与正文照常显示，档位信息在正文里
    expect(await screen.findByText("前端 dev server · 端口违规")).toBeTruthy();
    expect(
      screen.getByText("监听地址 0.0.0.0 不是 loopback（L1）"),
    ).toBeTruthy();
    // 类别标签
    expect(screen.getByText("服务告警")).toBeTruthy();
    // 只有「知道了」，没有别的决策按钮
    const btn = screen.getByRole("button", { name: "知道了" });
    await userEvent.click(btn);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("notifications_resolve", {
        id: "s1",
        status: "dismissed",
      }),
    );
  });
});
