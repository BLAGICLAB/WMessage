// WidgetApp 子模块：常量 + localStorage key。
// 由同目录其他子文件 + 主文件 import。

// 收起为触发条 / 展开为侧边面板
export const STRIP_W = 44;
export const STRIP_H = 220;
export const PANEL_W = 480;
export const PANEL_H = 560;
export const TOP_Y = 140; // 默认贴右缘的初始 Y

// 老板拍板：边框调整边界 + Splitter 上下限
export const PANEL_W_MIN = 400, PANEL_W_MAX = 800;
// C4-v2（2026-09-26 拍板）：底部放开 800→400。旧 MIN 800 > 默认 PANEL_H 560，
// 默认/存量高度落在非法区间、首次 resize 即被强钳到 800；MIN 必须不高于默认值。
// 400 为暂行下界，区间 400–900。
export const PANEL_H_MIN = 400, PANEL_H_MAX = 900;
export const CHAT_H_MIN = 120;
export const SPLITTER_H = 16;
export const DEFAULT_TASK_H = 280;

// C4-3：与 POS_KEY 同一命名空间 `wmessage-widget-`，kebab-case。仓库无 `:vN` 版本后缀
// 先例（grep 空）→ 不预设；后续若需迁移键，约定为 legacy/current 两键并存，不改本名。
export const SIZE_KEY = "wmessage-widget-size";
export const POS_KEY = "wmessage-widget-pos";
