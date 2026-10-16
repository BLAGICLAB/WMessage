// 内置预设头像注册表：ID 与 Rust profile.rs 的 AVATAR_IDS 一一对应
//（双端测试各锁 16 张；改任一侧必须同步另一侧）。图片打包进前端 assets，
// profile / 导出信封 / people 表全链路只传 ID，对端按 ID 解析回同一张图。

import a01 from "./assets/avatars/avatar-01.jpg";
import a02 from "./assets/avatars/avatar-02.jpg";
import a03 from "./assets/avatars/avatar-03.jpg";
import a04 from "./assets/avatars/avatar-04.jpg";
import a05 from "./assets/avatars/avatar-05.jpg";
import a06 from "./assets/avatars/avatar-06.jpg";
import a07 from "./assets/avatars/avatar-07.jpg";
import a08 from "./assets/avatars/avatar-08.jpg";
import a09 from "./assets/avatars/avatar-09.jpg";
import a10 from "./assets/avatars/avatar-10.jpg";
import a11 from "./assets/avatars/avatar-11.jpg";
import a12 from "./assets/avatars/avatar-12.jpg";
import a13 from "./assets/avatars/avatar-13.jpg";
import a14 from "./assets/avatars/avatar-14.jpg";
import a15 from "./assets/avatars/avatar-15.jpg";
import a16 from "./assets/avatars/avatar-16.jpg";

/** 内置头像 ID 全集；顺序即选择器网格顺序（行优先） */
export const AVATAR_IDS = [
  "avatar-01",
  "avatar-02",
  "avatar-03",
  "avatar-04",
  "avatar-05",
  "avatar-06",
  "avatar-07",
  "avatar-08",
  "avatar-09",
  "avatar-10",
  "avatar-11",
  "avatar-12",
  "avatar-13",
  "avatar-14",
  "avatar-15",
  "avatar-16",
] as const;

export type AvatarId = (typeof AVATAR_IDS)[number];

const AVATAR_SRC: Record<AvatarId, string> = {
  "avatar-01": a01,
  "avatar-02": a02,
  "avatar-03": a03,
  "avatar-04": a04,
  "avatar-05": a05,
  "avatar-06": a06,
  "avatar-07": a07,
  "avatar-08": a08,
  "avatar-09": a09,
  "avatar-10": a10,
  "avatar-11": a11,
  "avatar-12": a12,
  "avatar-13": a13,
  "avatar-14": a14,
  "avatar-15": a15,
  "avatar-16": a16,
};

/** ID → 打包资源 URL；未知 / 非法 ID（旧版文件名、手改垃圾值、外部信封）
 * 返回 null，调用方回退默认占位 */
export function avatarSrc(id: string | null | undefined): string | null {
  if (!id) return null;
  return (AVATAR_SRC as Record<string, string>)[id] ?? null;
}
