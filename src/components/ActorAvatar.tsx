import { useEffect, useState } from "react";
import {
  getProfileCache,
  loadProfile,
  subscribeProfile,
  type ProfileView,
} from "../profile";
import { avatarSrc } from "../avatars";
import botLogo from "../assets/main-logo.png";

/** 订阅资料（轻量 hook，组件级缓存） */
export function useProfile(): ProfileView | null {
  const [p, setP] = useState<ProfileView | null>(getProfileCache());
  useEffect(() => {
    let alive = true;
    loadProfile()
      .then((v) => {
        if (alive) setP(v);
      })
      // profile.ts 内已 handleCommandError 留痕；此处仅防 unhandled rejection，
      // 保持当前缓存（null 时走首字母兜底）
      .catch(() => {});
    const un = subscribeProfile(() => {
      if (alive) setP(getProfileCache());
    });
    return () => {
      alive = false;
      un();
    };
  }, []);
  return p;
}

/**
 * 归属头像：bot=true 显示机器人头像，否则用户头像。
 * 任务卡上只显示头像；鼠标悬停（title）显示姓名。
 * size 缺省 18（历史调用方不变）；周网格计划块按块高给 24/18/14。
 */
export function ActorAvatar({ bot, size = 18 }: { bot: boolean; size?: number }) {
  const profile = useProfile();
  const entry = bot ? profile?.bot : profile?.user;
  const name = entry?.name || (bot ? "机器人" : "我");
  const src = avatarSrc(entry?.avatarId);
  return (
    <span
      title={name}
      className="shrink-0 rounded-full overflow-hidden nm-inset flex items-center justify-center"
      style={{ width: size, height: size }}
    >
      {src ? (
        <img src={src} alt={name} className="w-full h-full object-cover" />
      ) : bot ? (
        <img src={botLogo} alt={name} className="w-full h-full object-cover" />
      ) : (
        <span
          className="leading-none text-[var(--t4)]"
          style={{ fontSize: Math.max(8, Math.round(size * 0.5)) }}
        >
          {name.charAt(0)}
        </span>
      )}
    </span>
  );
}
