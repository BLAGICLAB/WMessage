import { describe, expect, it } from "vitest";
import { AVATAR_IDS, avatarSrc } from "./avatars";

describe("内置头像注册表", () => {
  it("恰好 16 张、ID 唯一且形如 avatar-NN（与 Rust profile.rs AVATAR_IDS 对齐，改任一侧必须同步）", () => {
    expect(AVATAR_IDS).toHaveLength(16);
    expect(new Set(AVATAR_IDS).size).toBe(16);
    for (const id of AVATAR_IDS) {
      expect(id).toMatch(/^avatar-\d{2}$/);
    }
  });

  it("avatarSrc：合法 ID → 打包资源；未知/空值 → null（调用方回退默认占位）", () => {
    expect(avatarSrc("avatar-01")).toBeTruthy();
    expect(avatarSrc("avatar-16")).toBeTruthy();
    // 旧版拷贝文件名 / 编外 ID / 穿越串一律无资源
    expect(avatarSrc("avatar-17")).toBeNull();
    expect(avatarSrc("avatar-user.png")).toBeNull();
    expect(avatarSrc("../../x")).toBeNull();
    expect(avatarSrc(null)).toBeNull();
    expect(avatarSrc(undefined)).toBeNull();
    expect(avatarSrc("")).toBeNull();
  });
});
