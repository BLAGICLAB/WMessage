// 个人资料：用户/机器人 头像 + 姓名。与 Rust profile.rs 对应。
// profile-changed 事件由 Rust 广播（主窗口 + 挂件都监听）；本模块维护单例缓存。

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { handleCommandError } from "./lib/errorHandler";

type ProfileEntryView = {
  name: string;
  avatarDataUrl: string | null;
};

export type ProfileView = {
  user: ProfileEntryView;
  bot: ProfileEntryView;
};

let cache: ProfileView | null = null;
let loading: Promise<ProfileView> | null = null;
// 代际：force 发起新加载后递增，旧在飞 promise 的 resolve 不得覆盖新结果
let loadGen = 0;
const subs = new Set<() => void>();

function notify() {
  subs.forEach((cb) => cb());
}

/** 拉取资料（缓存命中直接返回；force=true 无视缓存与在飞请求，强制重新拉取） */
export async function loadProfile(force = false): Promise<ProfileView> {
  ensureProfileListen();
  if (cache && !force) return cache;
  // force 时旧在飞请求不得复用（否则显式刷新拿到的是未强制的结果）；
  // 旧 promise 晚到时由 loadGen 挡下，不覆盖 cache / 不重复 notify
  if (loading && !force) return loading;
  const gen = ++loadGen;
  const p = invoke<ProfileView>("profile_get")
    .then((v) => {
      if (gen === loadGen) {
        cache = v;
        notify();
      }
      return v;
    })
    .catch((e) => {
      // 加载失败：不静默吞错，让上层 try/catch 处理（SettingsPage ProfileRow 有 inline UI，
      // 这里只是把异常原样上抛，避免双重提示）
      handleCommandError(e, "profile_get", { silent: true });
      throw e;
    })
    .finally(() => {
      // 只清「自己这条链」——force 新链已接替时不动 loading
      if (loading === p) loading = null;
    });
  loading = p;
  return p;
}

/** 订阅资料变更；返回取消函数 */
export function subscribeProfile(cb: () => void): () => void {
  subs.add(cb);
  return () => {
    subs.delete(cb);
  };
}

export function getProfileCache(): ProfileView | null {
  return cache;
}

/** 改名 */
export async function setProfileName(kind: "user" | "bot", name: string): Promise<ProfileView> {
  return invoke<ProfileView>("profile_set_name", { kind, name }).then((v) => {
    cache = v;
    notify();
    return v;
  });
}

/** 上传头像（本地图片路径，Rust 拷贝进数据目录） */
export async function setProfileAvatar(kind: "user" | "bot", path: string): Promise<ProfileView> {
  return invoke<ProfileView>("profile_set_avatar", { kind, path }).then((v) => {
    cache = v;
    notify();
    return v;
  });
}

/** 移除头像（恢复默认占位） */
export async function removeProfileAvatar(kind: "user" | "bot"): Promise<ProfileView> {
  return invoke<ProfileView>("profile_remove_avatar", { kind }).then((v) => {
    cache = v;
    notify();
    return v;
  });
}

// Rust 广播的资料变更（换头像/改名后）→ 刷新缓存
let listening = false;
function ensureProfileListen() {
  if (listening) return;
  listening = true;
  listen<ProfileView>("profile-changed", (e) => {
    if (e.payload) {
      cache = e.payload;
      notify();
    }
  }).catch(() => {});
}
