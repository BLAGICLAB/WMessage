// 设置页排版原语：卡片与设置行。设置页 20+ 个面板共用同一套排版阶梯，
// 面板里不再各自写 text-lg / text-sm / text-xs 原子串。
//
// 阶梯只有三级，字体大小四档（small/standard/large/xlarge）靠
// [data-font-size] 覆盖 Tailwind 标准类放大——所以这里只用 text-lg / text-sm /
// text-xs 三个标准类，不用 text-[11px] 这类任意值：任意值类与标准类混排时
// 同一档里字号会错位（曾出现的 10/11/12px 混用就是这么来的）。
//
//  - set-title ：卡片标题（18px / 600）
//  - label     ：设置项名称（14px / 500，主文字色）
//  - desc      ：设置项说明（12px，弱文字色）

import type { ReactNode } from "react";

/** 卡片标题排版类（三级阶梯的第一级），仅本文件使用 */
const SET_TITLE_CLASS = "text-lg font-semibold text-[var(--t1)]";
/** 设置项名称排版类 */
export const SET_LABEL_CLASS = "text-sm font-medium text-[var(--t2)]";
/** 设置项说明排版类 */
export const SET_DESC_CLASS = "text-xs text-[var(--t5)]";

type SettingCardProps = {
  title: string;
  /** 卡片一句话说明；省略时不占行 */
  description?: ReactNode;
  /** 卡片头右侧动作区（按钮组等），纵向对齐顶边 */
  actions?: ReactNode;
  /** 正文内容 */
  children: ReactNode;
};

/** 设置卡：标题 + 可选说明 + 可选头右侧动作，正文自己带间距。 */
export function SettingCard({
  title,
  description,
  actions,
  children,
}: SettingCardProps) {
  return (
    <div className="nm-card p-5">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className={SET_TITLE_CLASS}>{title}</h2>
          {description && (
            <p className={`mt-1 ${SET_DESC_CLASS}`}>{description}</p>
          )}
        </div>
        {actions && <div className="flex items-center">{actions}</div>}
      </div>
      <div className="mt-4">{children}</div>
    </div>
  );
}

type SettingRowProps = {
  /** 设置项名称 */
  label: ReactNode;
  /** 说明文字：解释这个设置管什么、代价是什么 */
  description?: ReactNode;
  /** 顶部分隔线：本行与上一行属于不同设置组时打开 */
  divider?: boolean;
  /** 右侧控件 */
  children: ReactNode;
};

/** 设置行：左名称+说明、右控件。默认与上一行同组，divider 开分组线。 */
export function SettingRow({
  label,
  description,
  divider = false,
  children,
}: SettingRowProps) {
  return (
    <div
      className={`flex items-center justify-between gap-4 ${
        divider ? "border-t border-[var(--edge)] pt-3" : ""
      }`}
    >
      <div className="min-w-0">
        <p className={SET_LABEL_CLASS}>{label}</p>
        {description && (
          <p className={`mt-1 ${SET_DESC_CLASS}`}>{description}</p>
        )}
      </div>
      {children}
    </div>
  );
}
