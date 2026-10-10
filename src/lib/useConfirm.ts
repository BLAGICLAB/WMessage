import { useCallback } from "react";
import { ask } from "@tauri-apps/plugin-dialog";

/**
 * 破坏性操作二次确认（统一走 Tauri 2 原生 dialog plugin，不依赖 window.confirm）。
 *
 * 选择 OS 原生弹窗而非自建 React 组件：库内已有的 DeleteConfirmDialog 留给
 * 「带元数据预览/级联选项」的复杂场景（如删除提案时是否连带源记忆）——通用
 * 二次确认只用 11 个调用点，造组件是过度抽象。原生弹窗跨平台一致、无 z-index
 * 冲突、vitest 通过 vi.mock('@tauri-apps/plugin-dialog') 即可替换。
 *
 * 全部默认 `kind: 'warning'`，覆盖 11 个调用点（删模型/删厂商/删模板/
 * 删工作区/删 MCP/删任务/清 key/旋转 token/清审计/启 Python 沙箱/启用回滚过的提案/
 * 立即执行桌面清理）。**不可逆破坏性操作必须显式传 options**——本 hook 不再
 * 让调用点自己拼标题/按钮文案——文案统一在 confirmOptionDefaults。
 *
 * `await confirm('删除模板「foo」？')` 等价旧 `window.confirm('删除模板「foo」？')`，
 * 但弹出 OS 警告弹窗而非 WebView 同步阻塞的 confirm。
 */
export type ConfirmOptions = {
  /** 弹窗正文（多行用 \n） */
  message: string;
  /** 弹窗标题，默认「确认操作」 */
  title?: string;
  /** 严重性：warning（默认，覆盖所有破坏性）| info | error */
  kind?: "info" | "warning" | "error";
  /** 确认按钮文案，默认「确定」 */
  okLabel?: string;
  /** 取消按钮文案，默认「取消」 */
  cancelLabel?: string;
};

const DEFAULT_OPTIONS: Required<Omit<ConfirmOptions, "message">> = {
  title: "确认操作",
  kind: "warning",
  okLabel: "确定",
  cancelLabel: "取消",
};

export function useConfirm(): (
  messageOrOptions: string | ConfirmOptions,
) => Promise<boolean> {
  return useCallback(async (input) => {
    const options: ConfirmOptions =
      typeof input === "string" ? { message: input } : input;
    const merged = { ...DEFAULT_OPTIONS, ...options };
    try {
      return await ask(merged.message, {
        title: merged.title,
        kind: merged.kind,
        okLabel: merged.okLabel,
        cancelLabel: merged.cancelLabel,
      });
    } catch {
      // plugin-dialog 不可用（web/无头测试缺 mock）→ 默认拒绝（拒绝比误通过安全）
      return false;
    }
  }, []);
}
