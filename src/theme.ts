/** 主题管理：浅色/深色/跟随系统 三态，html.dark class + localStorage 持久化，双窗口 storage 事件同步 */

const THEME_KEY = "wmessage-theme";

export type ThemeSetting = "light" | "dark" | "system";
export type EffectiveTheme = "light" | "dark";

const mq = () =>
  typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : null;

export function getSetting(): ThemeSetting {
  try {
    const v = localStorage.getItem(THEME_KEY);
    if (v === "dark" || v === "system") return v;
    return "light";
  } catch {
    return "light";
  }
}

/** 计算实际生效主题（system → 跟随系统偏好） */
export function effectiveTheme(setting: ThemeSetting): EffectiveTheme {
  if (setting === "system") return mq()?.matches ? "dark" : "light";
  return setting;
}

function applyClass(dark: boolean) {
  document.documentElement.classList.toggle("dark", dark);
}

/** 持久化并套用设置 */
export function applySetting(setting: ThemeSetting) {
  applyClass(effectiveTheme(setting) === "dark");
  try {
    localStorage.setItem(THEME_KEY, setting);
  } catch {
    /* ignore */
  }
}

/** 三态循环：浅色 → 深色 → 跟随系统 → 浅色 */
export function cycleSetting(): ThemeSetting {
  const cur = getSetting();
  const next: ThemeSetting =
    cur === "light" ? "dark" : cur === "dark" ? "system" : "light";
  applySetting(next);
  return next;
}

/** 全局快捷键：浅/深快速来回切（system 态下按当前生效值取反，落到具体模式） */
export function toggleTheme(): ThemeSetting {
  const cur = getSetting();
  const next: ThemeSetting =
    effectiveTheme(cur) === "dark" ? "light" : "dark";
  applySetting(next);
  return next;
}

/** 监听其他窗口（挂件/主窗口）的主题变更并同步 */
export function subscribeTheme(onChange: (s: ThemeSetting) => void): () => void {
  const handler = (e: StorageEvent) => {
    if (e.key !== THEME_KEY) return;
    const next: ThemeSetting =
      e.newValue === "dark" || e.newValue === "system" ? e.newValue : "light";
    applyClass(effectiveTheme(next) === "dark");
    onChange(next);
  };
  window.addEventListener("storage", handler);
  return () => window.removeEventListener("storage", handler);
}

/** 跟随系统模式下，系统外观变化时自动切换并回调 */
export function subscribeSystem(onChange: (s: ThemeSetting) => void): () => void {
  const m = mq();
  if (!m) return () => {};
  const handler = () => {
    if (getSetting() === "system") {
      applyClass(effectiveTheme("system") === "dark");
      onChange(getSetting());
    }
  };
  // 兼容旧 WebView：addEventListener 不存在时退回 addListener（清理函数此前为空，泄漏监听）
  if (typeof m.addEventListener === "function") {
    m.addEventListener("change", handler);
    return () => m.removeEventListener("change", handler);
  }
  const legacy = m as MediaQueryList & {
    addListener: (f: () => void) => void;
    removeListener?: (f: () => void) => void;
  };
  legacy.addListener(handler);
  return () => legacy.removeListener?.(handler);
}
