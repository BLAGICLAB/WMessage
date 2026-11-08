// ChatPanel 子模块：输入卡底栏的逐级图标化定档（推理强度 → 授权 → 模型）。
//
// 按实际内容定档而不写死阈值：模型名长短能差好几倍，固定阈值要么过早把短名
// 图标化，要么让长名挤压重叠。量两样东西——
//   可用宽度：输入卡内容盒（即底栏 flex 行的实际宽），ResizeObserver 观测。
//     量卡片不量窗口：左右侧栏开合改变的是卡片宽，窗口断点量不出这个落差。
//   文字宽度：三枚控件文字态的文案，canvas measureText 现量；字体取自按钮的
//     计算样式（字号设置覆盖的是文字类，读计算样式才跟得上）。
// 宽度与字体在观测回调里一并记下（回调里读 ref、写 state 都不落在渲染期），
// 档位在渲染期按「宽度 + 字体 + 当前文案」纯算：换模型、改档位的那次渲染
// 当场重新定档，不经过 effect，也不会先错一帧再纠正。
//
// 容器隐藏量到 0（聊天页切走是隐藏不卸载）、环境缺 ResizeObserver（jsdom）
// 或拿不到 canvas：维持全带文字的 0 档，不抛错。

import { useEffect, useMemo, useState } from "react";
import type { RefObject } from "react";
import { labelExtra, pickIconLevel, type IconLevel } from "./chatLayout";

/** 模型名文字态的显示上限（与模型 chip 文字的 max-w-[150px] 同值） */
const MODEL_LABEL_MAX_W = 150;

/** 测字用的 2D 上下文，首次用到时才建。拿不到时不缓存 null：下次再试的
 *  代价只是一次空调用，缓存了反而让后来就绪的环境永远量不到 */
let measureCtx: CanvasRenderingContext2D | null = null;

function measureText(text: string, font: string): number {
  if (!measureCtx) {
    measureCtx = document.createElement("canvas").getContext("2d");
  }
  if (!measureCtx) return 0;
  measureCtx.font = font;
  return measureCtx.measureText(text).width;
}

function fontOf(el: HTMLElement | null): string {
  if (!el) return "";
  const cs = getComputedStyle(el);
  // 个别引擎不回填 font 简写：用分项拼一份等价的
  return (
    cs.font || `${cs.fontStyle} ${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`
  );
}

type Geometry = {
  avail: number;
  fonts: { effort: string; perm: string; model: string };
};

/**
 * 观测输入卡宽度，按三枚控件当前文案定出底栏的图标化档位。
 * 三个按钮 ref 在文字态与图标态下指向同一个元素（只换内容不换节点）。
 */
export function useComposerIconLevel(
  cardRef: RefObject<HTMLElement | null>,
  effortRef: RefObject<HTMLElement | null>,
  permRef: RefObject<HTMLElement | null>,
  modelRef: RefObject<HTMLElement | null>,
  texts: { effort: string; perm: string; model: string },
): IconLevel {
  const [geom, setGeom] = useState<Geometry | null>(null);
  useEffect(() => {
    const card = cardRef.current;
    if (!card || typeof ResizeObserver !== "function") return;
    const ro = new ResizeObserver((entries) => {
      const avail = entries[entries.length - 1]?.contentRect.width ?? 0;
      if (avail <= 0) return;
      setGeom({
        avail,
        fonts: {
          effort: fontOf(effortRef.current),
          perm: fontOf(permRef.current),
          model: fontOf(modelRef.current),
        },
      });
    });
    ro.observe(card);
    return () => ro.disconnect();
  }, [cardRef, effortRef, permRef, modelRef]);
  return useMemo(() => {
    if (!geom) return 0;
    const { avail, fonts } = geom;
    return pickIconLevel(avail, {
      effort: labelExtra(measureText(texts.effort, fonts.effort)),
      perm: labelExtra(measureText(texts.perm, fonts.perm)),
      model: labelExtra(
        Math.min(measureText(texts.model, fonts.model), MODEL_LABEL_MAX_W),
      ),
    });
  }, [geom, texts.effort, texts.perm, texts.model]);
}
