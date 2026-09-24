// WidgetApp 子模块：WidgetSize / Anchor 类型 + 持久化 + 屏幕尺寸探测 + 全局拖动状态。
//
// `widgetDragActive` 是模块级 mutable：ResizeEdge / SplitBar 写、WidgetApp collapse 读。
// 必须放模块级（不能用闭包）才能让几个组件共享同一份拖动期间标志。

import { currentMonitor } from "@tauri-apps/api/window";

import {
  DEFAULT_TASK_H,
  PANEL_H_MAX,
  PANEL_H_MIN,
  PANEL_W_MAX,
  PANEL_W_MIN,
  POS_KEY,
  SIZE_KEY,
  STRIP_W,
} from "./constants";

// ────────────────────────── 尺寸 + 位置类型 ──────────────────────────

export interface WidgetSize {
  w: number;
  h: number;
}

const EDGES = ["right", "left", "top", "float"] as const;

export type Edge = (typeof EDGES)[number];

export interface Anchor {
  x: number;
  y: number;
  edge: Edge;
}

// ────────────────────────── 模块级拖动状态 ──────────────────────────

// 拖拽进行中标记:拖动调整期间禁止 mouseleave 触发折叠(防止拖到一半「缩回去」)
export let widgetDragActive = false;
export function setWidgetDragActive(v: boolean) {
  widgetDragActive = v;
}

// ────────────────────────── 尺寸持久化 ──────────────────────────

export function loadSize(): WidgetSize | null {
  try {
    const raw = localStorage.getItem(SIZE_KEY);
    if (raw) {
      const s = JSON.parse(raw) as WidgetSize;
      if (
        s.w >= PANEL_W_MIN && s.w <= PANEL_W_MAX &&
        s.h >= PANEL_H_MIN && s.h <= PANEL_H_MAX
      ) return s;
    }
  } catch {
    /* ignore */
  }
  return null;
}

export function saveSize(s: WidgetSize) {
  try { localStorage.setItem(SIZE_KEY, JSON.stringify(s)); } catch { /* ignore */ }
}

// 从 CSS 变量读/写任务区高度(Splitter 拖动用,不入 React state)
// 注意:拖到顶部时值为 "0px",0 是合法状态,不能当无效值回退成默认高度
export function getTaskH(): number {
  const raw = document.documentElement.style.getPropertyValue("--task-h");
  if (!raw) return DEFAULT_TASK_H; // 从未设置过(首次)
  const v = parseInt(raw, 10);
  return Number.isFinite(v) ? Math.max(0, v) : DEFAULT_TASK_H;
}

export function setTaskH(h: number) {
  document.documentElement.style.setProperty("--task-h", h + "px");
}

// ────────────────────────── 位置持久化 ──────────────────────────

export function loadAnchor(): Anchor | null {
  try {
    const raw = localStorage.getItem(POS_KEY);
    if (!raw) return null;
    // 形状校验（对齐 loadSize 策略：坏值返 null，不清除不落盘）：
    // object 非 null + x/y 为 finite number + edge 在 allow-list
    const p: unknown = JSON.parse(raw);
    if (p !== null && typeof p === "object" && !Array.isArray(p)) {
      const a = p as Record<string, unknown>;
      if (
        typeof a.x === "number" && Number.isFinite(a.x) &&
        typeof a.y === "number" && Number.isFinite(a.y) &&
        typeof a.edge === "string" && (EDGES as readonly string[]).includes(a.edge)
      ) {
        return { x: a.x, y: a.y, edge: a.edge as Edge };
      }
    }
  } catch {
    /* ignore */
  }
  return null;
}

export function saveAnchor(a: Anchor) {
  try {
    localStorage.setItem(POS_KEY, JSON.stringify(a));
  } catch {
    /* ignore */
  }
}

// ────────────────────────── 屏幕尺寸探测 ──────────────────────────

export async function screenSize(): Promise<{ w: number; h: number }> {
  try {
    const m = await currentMonitor();
    if (m)
      return { w: m.size.width / m.scaleFactor, h: m.size.height / m.scaleFactor };
  } catch {
    /* ignore */
  }
  return { w: 1920, h: 1080 };
}

// ────────────────────────── 贴边检测 ──────────────────────────

// 按窗口当前矩形推算：贴边检测（右/左/顶，24px 容差）+ 触发条锚点
// 返回扁平 Anchor（与导出接口同形，调用方直接用，不再有 anchor 包装层）
export function anchorFromRect(
  x: number,
  y: number,
  w: number,
  sw: number,
): Anchor {
  const MARGIN = 24;
  if (x + w >= sw - MARGIN) return { x: sw - STRIP_W, y, edge: "right" };
  if (x <= MARGIN) return { x: 0, y, edge: "left" };
  if (y <= MARGIN) return { x, y: 0, edge: "top" };
  return { x, y, edge: "float" };
}
