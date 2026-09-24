import { describe, expect, it, vi, beforeEach } from "vitest";

// profile.ts 是单例模块（cache/loading/loadGen 模块态）——每测试 resetModules + 动态 import 隔离
const invokeMock = vi.hoisted(() => vi.fn());
const listenMock = vi.hoisted(() => vi.fn());
const handleCommandErrorMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("./lib/errorHandler", () => ({
  handleCommandError: handleCommandErrorMock,
}));

type ProfileView = {
  user: { name: string; avatarDataUrl: string | null };
  bot: { name: string; avatarDataUrl: string | null };
};
const mkView = (name: string): ProfileView => ({
  user: { name, avatarDataUrl: null },
  bot: { name: "bot", avatarDataUrl: null },
});

const importProfile = () => import("./profile");

describe("profile 缓存 / force 语义", () => {
  beforeEach(() => {
    vi.resetModules();
    invokeMock.mockReset();
    listenMock.mockReset();
    listenMock.mockResolvedValue(() => {});
    handleCommandErrorMock.mockReset();
  });

  it("非 force：在飞去重——并发两调只发一次 invoke", async () => {
    invokeMock.mockResolvedValue(mkView("v1"));
    const { loadProfile } = await importProfile();
    const [a, b] = await Promise.all([loadProfile(), loadProfile()]);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(a.user.name).toBe("v1");
    expect(b.user.name).toBe("v1");
  });

  it("force：在飞期间 force=true 发起新 invoke（不返回旧 promise）", async () => {
    const resolvers: ((v: ProfileView) => void)[] = [];
    invokeMock.mockImplementation(
      () => new Promise<ProfileView>((r) => resolvers.push(r))
    );
    const { loadProfile } = await importProfile();
    const p1 = loadProfile(); // gen1 在飞
    const p2 = loadProfile(true); // force → 必须新发 invoke
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(p1).not.toBe(p2);
    // 收尾：force 结果先生效
    resolvers[1](mkView("fresh"));
    await expect(p2).resolves.toMatchObject({ user: { name: "fresh" } });
    resolvers[0](mkView("stale"));
    await p1;
  });

  it("旧在飞 resolve 晚于 force 完成 → cache/notify 不被旧值覆盖", async () => {
    const resolvers: ((v: ProfileView) => void)[] = [];
    invokeMock.mockImplementation(
      () => new Promise<ProfileView>((r) => resolvers.push(r))
    );
    const { loadProfile, getProfileCache, subscribeProfile } =
      await importProfile();
    const notifySpy = vi.fn();
    subscribeProfile(notifySpy);
    const p1 = loadProfile(); // gen1
    const p2 = loadProfile(true); // gen2
    resolvers[1](mkView("fresh")); // 新的先回
    await p2;
    resolvers[0](mkView("stale")); // 旧的后回——不得覆盖
    await p1;
    expect(getProfileCache()?.user.name).toBe("fresh");
    expect(notifySpy).toHaveBeenCalledTimes(1);
  });

  it("force：有缓存时也重新拉取", async () => {
    invokeMock.mockResolvedValue(mkView("v1"));
    const { loadProfile } = await importProfile();
    await loadProfile();
    invokeMock.mockResolvedValue(mkView("v2"));
    const v = await loadProfile(true);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(v.user.name).toBe("v2");
  });

  it("加载失败：错误上抛 + handleCommandError silent 留痕（不静默吞）", async () => {
    const err = new Error("db locked");
    invokeMock.mockRejectedValue(err);
    const { loadProfile } = await importProfile();
    await expect(loadProfile()).rejects.toBe(err);
    expect(handleCommandErrorMock).toHaveBeenCalledWith(err, "profile_get", {
      silent: true,
    });
  });
});
