// 工作流 AI 拆解提示词（W2-DECOMPOSE，设计 §6.3/§10）：
// 用户可编辑的是「指引段」，随 workflow_decompose invoke 上行；
// 「输出契约段」在 Rust workflow_decompose.rs 代码硬拼追加，用户不可见不可改。
// 纯前端偏好，走 localStorage；DEFAULT 必须与 Rust DEFAULT_DECOMPOSE_GUIDANCE 保持一致。

const GUIDANCE_KEY = "wm.workflow.decomposeGuidance";

const DEFAULT_DECOMPOSE_GUIDANCE = `你是工作流拆解专家。把用户的目标拆解为一组可执行的任务卡。
拆解原则：
- 每张卡是一个明确的、可独立交付的步骤，粒度适中：不拆成太碎的分钟级动作，也不留"把所有事做完"的空泛大卡
- 卡的 note 写清楚：做什么、产出什么（下游卡会引用上游产出）
- 有顺序或数据依赖的卡用 dependsOn 表达先后，无依赖的卡并行
- 日常目标通常 3~10 张卡即可覆盖`;

export function getDecomposeGuidance(): string {
  try {
    return localStorage.getItem(GUIDANCE_KEY) ?? DEFAULT_DECOMPOSE_GUIDANCE;
  } catch {
    return DEFAULT_DECOMPOSE_GUIDANCE;
  }
}

export function setDecomposeGuidance(v: string): void {
  try {
    localStorage.setItem(GUIDANCE_KEY, v);
  } catch {
    // localStorage 不可用：仅本次会话内存生效
  }
}

export function resetDecomposeGuidance(): string {
  try {
    localStorage.removeItem(GUIDANCE_KEY);
  } catch {
    // 同上
  }
  return DEFAULT_DECOMPOSE_GUIDANCE;
}
