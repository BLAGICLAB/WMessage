/** 聊天气泡外观：用户气泡颜色 + 机器人气泡材质。
 *  纯前端视觉偏好，模式同 theme.ts：html 内联 CSS 变量 / data 属性即时生效 +
 *  localStorage 持久化，双窗口（主窗/挂件）storage 事件同步，零后端改动。
 *  main.css 消费：--bubble-user-bg/--bubble-user-fg/.user-chip 三变量 + data-bubble-bot。 */

const KEY = "wmessage-bubble-style";

type BotBubbleMaterial = "card" | "flat" | "inset";

export type BubbleStyle = {
  /** 用户气泡底色（#rrggbb）。null = 跟随主题（用 --brand，深浅色自动切换） */
  userBg: string | null;
  /** 机器人气泡材质：卡片（raised 底+边框+投影，默认）/ 扁平（无容器纯文本）/ 凹陷（内嵌灰底） */
  botMaterial: BotBubbleMaterial;
};

export const BUBBLE_STYLE_DEFAULT: BubbleStyle = { userBg: null, botMaterial: "card" };

/** 用户气泡颜色预设：全部对白字 ≥4.5:1（AA）；
 *  「跟随主题」（null）不在预设里，由设置卡单独渲染成首个色块 */
export const BUBBLE_USER_COLOR_PRESETS = [
  { value: "#3563b0", label: "蓝" },
  { value: "#1f7a6d", label: "青绿" },
  { value: "#35854a", label: "绿" },
  { value: "#b05f2f", label: "陶橙" },
  { value: "#b04a6a", label: "玫红" },
  { value: "#6d5aa8", label: "紫" },
  { value: "#4a5568", label: "石墨" },
] as const;

/** 机器人气泡材质选项（设置卡分段按钮 + 文案） */
export const BUBBLE_BOT_MATERIAL_OPTIONS = [
  { value: "card", label: "卡片" },
  { value: "flat", label: "扁平" },
  { value: "inset", label: "凹陷" },
] as const;

const HEX_RE = /^#[0-9a-fA-F]{6}$/;
const MATERIALS: readonly BotBubbleMaterial[] = BUBBLE_BOT_MATERIAL_OPTIONS.map((o) => o.value);

/** 未知存储归一到合法值：任何脏数据都回退默认，不让设置页/启动路径抛错 */
function normalize(input: unknown): BubbleStyle {
  if (!input || typeof input !== "object") return { ...BUBBLE_STYLE_DEFAULT };
  const o = input as { userBg?: unknown; botMaterial?: unknown };
  return {
    userBg: typeof o.userBg === "string" && HEX_RE.test(o.userBg) ? o.userBg : null,
    botMaterial: MATERIALS.includes(o.botMaterial as BotBubbleMaterial)
      ? (o.botMaterial as BotBubbleMaterial)
      : "card",
  };
}

export function getBubbleStyle(): BubbleStyle {
  try {
    const raw = localStorage.getItem(KEY);
    return raw ? normalize(JSON.parse(raw)) : { ...BUBBLE_STYLE_DEFAULT };
  } catch {
    return { ...BUBBLE_STYLE_DEFAULT };
  }
}

/** 依底色相对亮度自动选气泡文字色（Telegram/Discord 式自动反白）：
 *  亮度阈值 0.2 —— 低于它的底色配白字 ≥4.5:1，高于它配深字。 */
export function pickUserFg(bg: string): string {
  if (!HEX_RE.test(bg)) return "#ffffff";
  const lin = (h: string) => {
    const v = parseInt(h, 16) / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  const lum =
    0.2126 * lin(bg.slice(1, 3)) + 0.7152 * lin(bg.slice(3, 5)) + 0.0722 * lin(bg.slice(5, 7));
  return lum > 0.2 ? "#171d2b" : "#ffffff";
}

/** 只动 DOM（不落盘）：订阅方/启动路径用，避免双窗口互相写引发 storage 事件回声 */
export function applyBubbleStyleDom(style: BubbleStyle): BubbleStyle {
  const s = normalize(style);
  const root = document.documentElement;
  if (s.userBg) {
    const fg = pickUserFg(s.userBg);
    const darkText = fg !== "#ffffff";
    root.style.setProperty("--bubble-user-bg", s.userBg);
    root.style.setProperty("--bubble-user-fg", fg);
    // 附件芯片跟随文字色：白字配白透芯片、深字配暗透芯片（深浅色模式同值，自定义底色不随主题变）
    root.style.setProperty(
      "--bubble-chip-bg",
      darkText ? "rgba(23, 32, 51, 0.1)" : "rgba(255, 255, 255, 0.16)",
    );
    root.style.setProperty(
      "--bubble-chip-border",
      darkText ? "rgba(23, 32, 51, 0.2)" : "rgba(255, 255, 255, 0.3)",
    );
    root.style.setProperty("--bubble-chip-fg", darkText ? "#26304a" : "rgba(255, 255, 255, 0.95)");
  } else {
    for (const v of [
      "--bubble-user-bg",
      "--bubble-user-fg",
      "--bubble-chip-bg",
      "--bubble-chip-border",
      "--bubble-chip-fg",
    ]) {
      root.style.removeProperty(v);
    }
  }
  root.dataset.bubbleBot = s.botMaterial;
  return s;
}

/** 应用并持久化（设置卡交互入口；其他窗口经 storage 事件走 applyBubbleStyleDom 同步） */
export function applyBubbleStyle(style: BubbleStyle): BubbleStyle {
  const s = applyBubbleStyleDom(style);
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    /* ignore */
  }
  return s;
}

/** 恢复默认（跟随主题 + 卡片材质） */
export function resetBubbleStyle(): BubbleStyle {
  return applyBubbleStyle({ ...BUBBLE_STYLE_DEFAULT });
}

/** 监听其他窗口（主窗/挂件）的气泡样式变更并同步到本窗口 DOM */
export function subscribeBubbleStyle(onChange: (s: BubbleStyle) => void): () => void {
  const handler = (e: StorageEvent) => {
    if (e.key !== KEY) return;
    onChange(applyBubbleStyleDom(getBubbleStyle()));
  };
  window.addEventListener("storage", handler);
  return () => window.removeEventListener("storage", handler);
}
