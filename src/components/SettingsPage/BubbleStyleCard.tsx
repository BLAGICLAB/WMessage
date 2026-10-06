// SettingsPage 子模块：聊天气泡外观卡（基础设置分区）。
// 用户气泡颜色（跟随主题/预设色板/自定义拾色器）+ 机器人气泡材质三选，
// 即点即生效（html CSS 变量 / data 属性）+ localStorage 持久化（src/bubbleStyle.ts），
// 其他窗口经 storage 事件同步。卡内迷你预览直接用真 .chat-bubble-* 类——
// 全局 CSS 变量一变，预览即时跟着变，无需单独的预览状态。

import { useEffect, useState } from "react";
import {
  applyBubbleStyle,
  BUBBLE_BOT_MATERIAL_OPTIONS,
  BUBBLE_USER_COLOR_PRESETS,
  getBubbleStyle,
  resetBubbleStyle,
  subscribeBubbleStyle,
  type BubbleStyle,
} from "../../bubbleStyle";

export function BubbleStyleCard() {
  const [style, setStyle] = useState<BubbleStyle>(() => getBubbleStyle());

  // 其他窗口（主窗/挂件）改了样式 → storage 事件同步回本页控件
  useEffect(() => subscribeBubbleStyle(setStyle), []);

  const update = (patch: Partial<BubbleStyle>) =>
    setStyle((prev) => applyBubbleStyle({ ...prev, ...patch }));

  const swatchRing =
    "ring-2 ring-[var(--brand)] ring-offset-2 ring-offset-[var(--surface)]";

  return (
    <div className="nm-card p-5">
      <h2 className="text-lg font-semibold text-[var(--t1)]">聊天气泡</h2>
      <div className="mt-4 space-y-3">
        {/* 实时预览：直接用真气泡类，上面的设置一变全局 CSS 变量就变 */}
        <div className="nm-inset space-y-2 rounded-[var(--r-md)] p-3">
          <div className="ml-auto max-w-[80%]">
            <div className="chat-bubble chat-bubble-user text-xs leading-relaxed">
              帮我把今天的任务标记完成
            </div>
          </div>
          <div className="mr-auto max-w-full">
            <div className="chat-bubble chat-bubble-bot text-xs leading-relaxed">
              好的，已标记 3 个任务完成 ✅
            </div>
          </div>
        </div>

        {/* 用户气泡颜色：跟随主题 / 预设色板 / 自定义拾色器 */}
        <div>
          <p className="text-sm font-medium text-[var(--t2)]">用户气泡颜色</p>
          <div className="mt-2 flex flex-wrap items-center gap-2">
            <button
              type="button"
              title="跟随主题"
              aria-label="用户气泡颜色：跟随主题"
              className={`h-6 w-6 shrink-0 rounded-full border border-[var(--edge-strong)] ${
                style.userBg === null ? swatchRing : ""
              }`}
              style={{
                background:
                  "linear-gradient(135deg, var(--brand) 50%, var(--surface-raised) 50%)",
              }}
              onClick={() => update({ userBg: null })}
            />
            {BUBBLE_USER_COLOR_PRESETS.map((p) => (
              <button
                key={p.value}
                type="button"
                title={p.label}
                aria-label={`用户气泡颜色：${p.label}`}
                className={`h-6 w-6 shrink-0 rounded-full border border-black/10 ${
                  style.userBg === p.value ? swatchRing : ""
                }`}
                style={{ backgroundColor: p.value }}
                onClick={() => update({ userBg: p.value })}
              />
            ))}
            <label
              title="自定义颜色"
              className="relative inline-flex h-6 w-6 shrink-0 cursor-pointer items-center justify-center rounded-full border border-dashed border-[var(--edge-strong)] text-[var(--t4)]"
            >
              +
              <input
                type="color"
                aria-label="用户气泡颜色：自定义"
                value={style.userBg ?? "#5f6f94"}
                onChange={(e) => update({ userBg: e.target.value })}
                className="absolute inset-0 h-full w-full cursor-pointer opacity-0"
              />
            </label>
          </div>
        </div>

        {/* 机器人气泡材质：卡片 / 扁平 / 凹陷（视觉规则在 main.css data-bubble-bot） */}
        <div>
          <p className="text-sm font-medium text-[var(--t2)]">机器人气泡样式</p>
          <div className="mt-2 flex gap-1">
            {BUBBLE_BOT_MATERIAL_OPTIONS.map((o) => (
              <button
                key={o.value}
                type="button"
                className={`flex-1 px-2 py-1.5 text-xs text-[var(--t3)] ${
                  style.botMaterial === o.value ? "nm-inset" : "nm-outset"
                }`}
                onClick={() => update({ botMaterial: o.value })}
              >
                {o.label}
              </button>
            ))}
          </div>
          <p className="mt-1.5 text-xs text-[var(--t5)]">
            卡片＝白卡带边框投影；扁平＝无底色纯文本；凹陷＝内嵌灰底。
          </p>
        </div>

        {/* 恢复默认 */}
        <div className="flex items-center justify-between gap-4 border-t border-[var(--edge)] pt-3">
          <p className="text-xs text-[var(--t5)]">
            气泡样式即时生效并自动保存，主窗口与挂件同步。
          </p>
          <button
            type="button"
            className="nm-outset shrink-0 rounded-[var(--r-sm)] px-3 py-1 text-xs text-[var(--t3)]"
            onClick={() => setStyle(resetBubbleStyle())}
          >
            恢复默认
          </button>
        </div>
      </div>
    </div>
  );
}
