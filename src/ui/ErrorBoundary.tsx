import { Component, type ErrorInfo, type ReactNode } from "react";
import { RotateCcw, TriangleAlert } from "lucide-react";

/** ErrorBoundary（ 自 App.tsx 提取共享）：任何子组件抛错时不再 unmount 变白， *  捕到错误显示堆栈 + 「重试」按钮重置 state。主窗口与挂件窗口共用（挂件此前
 *  无边界，浏览器/异常路径下白屏——U1 登记项）。class component 必需（hooks
 *  写法目前 React 还没稳定 API）。 */
export class ErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  override state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  override componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("[ErrorBoundary]", error, info.componentStack);
  }
  reset = () => this.setState({ error: null });
  override render() {
    const err = this.state.error;
    if (err) {
      // JS 允许 throw 非 Error 值（React 类型签名拦不住运行时）：统一规整成可展示文本。
      // 规整本身不许再抛（toString 抛错的对象会炸掉边界自己的渲染 → 白屏）：
      let message = "";
      try {
        message = err instanceof Error ? err.message : String(err ?? "未知错误");
      } catch {
        message = "未知错误";
      }
      const stack = err instanceof Error ? err.stack : undefined;
      return (
        <div className="min-h-screen bg-[var(--bg)] p-6 flex items-center justify-center">
          <div className="nm-card p-6 max-w-2xl">
            <p className="text-base font-semibold text-[var(--danger)] mb-2">
              <TriangleAlert size={14} aria-hidden className="inline-block align-[-2px]" /> 窗口发生错误
            </p>
            <p className="text-sm text-[var(--t2)] mb-2">{message}</p>
            <pre className="text-[10px] text-[var(--t4)] whitespace-pre-wrap overflow-auto max-h-64 bg-[var(--inset)] p-3 rounded-xl mb-3">
              {stack}
            </pre>
            <button
              className="nm-btn px-4 py-1.5 text-sm text-[var(--t2)] inline-flex items-center gap-1.5 whitespace-nowrap"
              onClick={this.reset}
            >
              <RotateCcw size={13} aria-hidden className="inline-block align-[-2px]" /> 重试
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
