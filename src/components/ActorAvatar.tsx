import { useEffect, useState } from "react";
import { getProfileCache, loadProfile, subscribeProfile, type ProfileView } from "../profile";
import botLogo from "../assets/main-logo.png";

/** 订阅资料（轻量 hook，组件级缓存） */
export function useProfile(): ProfileView | null {
  const [p, setP] = useState<ProfileView | null>(getProfileCache());
  useEffect(() => {
    let alive = true;
    loadProfile().then((v) => {
      if (alive) setP(v);
    });
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
 */
export function ActorAvatar({ bot }: { bot: boolean }) {
  const profile = useProfile();
  const entry = bot ? profile?.bot : profile?.user;
  const name = entry?.name || (bot ? "机器人" : "我");
  const src = entry?.avatarDataUrl;
  return (
    <span
      title={name}
      className="shrink-0 w-[18px] h-[18px] rounded-full overflow-hidden nm-inset flex items-center justify-center"
    >
      {src ? (
        <img src={src} alt={name} className="w-full h-full object-cover" />
      ) : bot ? (
        <img src={botLogo} alt={name} className="w-full h-full object-cover" />
      ) : (
        <span className="text-[9px] leading-none text-[var(--t4)]">{name.charAt(0)}</span>
      )}
    </span>
  );
}
