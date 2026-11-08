// ChatPanel 子模块：聊天区布局尺寸的单一真相源 + 输入卡底栏的图标化定档。
//
// 这些宽度不是随手写的视觉参数，而是一条推导链：底栏全图标行的宽度推出
// 聊天中栏的最小宽度，中栏最小宽度加上两侧栏与主导航推出窗口最小宽度
// （tauri.conf.json 的 minWidth）。任何一环改了不同步，窗口最窄时中栏就装
// 不下底栏、控件互相压住——所以组件里的宽度都从这里 import，窗口配置由
// chatLayout.test 对拍。
//
// 宽度类基于 rem，而字号设置（data-font-size）只覆盖文字类、不缩放根字号，
// 所以下面的 px 值在三档字号下都成立；随字号变的只有文字宽度，那部分由
// useComposerIconLevel 按计算样式现量。

import { NAV_W } from "../../lib/navWidth";

/** 底栏行内间距 */
export const COMPOSER_GAP = 4;
/** ＋ 附件钮宽度 */
export const COMPOSER_PLUS_W = 28;
/** 授权 / 模型 / 推理强度三枚下拉钮的图标态宽度：图标 + 下拉箭头 + 内边距，
 *  大字号下箭头变宽也放得下（内容约 24–36px） */
export const COMPOSER_CHIP_ICON_W = 40;
/** 发送 / 停止一体键宽度 */
export const COMPOSER_SEND_W = 32;
/** 底栏六个弹性项（＋ / 弹性空隙 / 🛡 / 🧠 / ⚡ / 发送）之间的五个间距 */
const COMPOSER_ROW_GAPS = 5;

/** 全图标行宽：控件全部是图标态时底栏一行的宽度 */
export const COMPOSER_ICON_ROW_W =
  COMPOSER_PLUS_W +
  3 * COMPOSER_CHIP_ICON_W +
  COMPOSER_SEND_W +
  COMPOSER_ROW_GAPS * COMPOSER_GAP;

/** 输入卡横向内边距 + 边框（p-2.5 两侧 + 1px 边框两侧） */
const COMPOSER_CARD_PAD_X = 2 * 10 + 2 * 1;

/** 聊天中栏最小宽度：恰好装得下全图标的底栏 */
export const COMPOSER_MIN_W = COMPOSER_ICON_ROW_W + COMPOSER_CARD_PAD_X;

/** 左侧会话栏宽度 */
export const SESSION_RAIL_W = 224;
/** 右侧上下文栏最小宽度 */
export const CONTEXT_RAIL_MIN_W = 240;
/** 主区横向内边距（App 主区的 p-6 两侧） */
const MAIN_PAD_X = 2 * 24;
/** 聊天区三栏之间的间距（ChatPanel 根的 gap-4） */
const CHAT_COL_GAP = 16;

/** 窗口最小宽度：两侧栏全展开时，中栏仍守得住 COMPOSER_MIN_W */
export const CHAT_FULL_LAYOUT_MIN_W =
  NAV_W +
  MAIN_PAD_X +
  SESSION_RAIL_W +
  2 * CHAT_COL_GAP +
  COMPOSER_MIN_W +
  CONTEXT_RAIL_MIN_W;

/** 带文字控件从图标态换成文字态多占的宽度：文字 + 它与图标之间的间距。
 *  取偏保守的值（不小于实际差值），宁可早一点图标化也不让控件压住 */
export function labelExtra(textW: number): number {
  return textW + COMPOSER_GAP;
}

/** 底栏图标化档位：0 = 全带文字；1 = 推理强度图标化；2 = 授权也图标化；
 *  3 = 全图标。先让信息量最小的「默认 / 高」，模型名信息量最大、最后才让 */
export type IconLevel = 0 | 1 | 2 | 3;

/**
 * 按可用宽度定档：从 0 档起取第一个放得下的。extras 是三枚控件文字态的
 * 额外占宽（labelExtra 的结果）。可用宽度连全图标行都不够时落 3 档——
 * 中栏最小宽度保证这种情况不会真的出现。
 */
export function pickIconLevel(
  avail: number,
  extras: { effort: number; perm: number; model: number },
): IconLevel {
  const base = COMPOSER_ICON_ROW_W;
  if (avail >= base + extras.effort + extras.perm + extras.model) return 0;
  if (avail >= base + extras.perm + extras.model) return 1;
  if (avail >= base + extras.model) return 2;
  return 3;
}
