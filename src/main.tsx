import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";

/**
 * 把前端错误回传给后端（打进终端日志）。
 *
 * React 崩了会导致 `#root` 为空 = 整窗白屏，webview 里什么都看不到。
 * 没有这条通道，白屏只能靠猜。
 */
function report(scope: string, err: unknown) {
  const detail =
    err instanceof Error ? `${err.message}\n${err.stack ?? ''}` : String(err);
  const text = `${scope}: ${detail}`;
  console.error(text);
  invoke('log_frontend_error', { message: text }).catch(() => {
    /* 连上报都失败就没辙了，至少 console 里有 */
  });
}

window.addEventListener('error', (e) => report('window.onerror', e.error ?? e.message));
window.addEventListener('unhandledrejection', (e) => report('unhandledrejection', e.reason));

/** 渲染期异常兜底：显示错误而不是白屏。 */
class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    report('render', new Error(`${error.message}\n组件栈:${info.componentStack ?? ''}`));
  }

  render() {
    if (this.state.error) {
      return (
        <div className="fatal">
          <h3>界面出错了</h3>
          <pre>{this.state.error.message}</pre>
        </div>
      );
    }
    return this.props.children;
  }
}

try {
  const root = document.getElementById("root");
  if (!root) throw new Error('找不到 #root 节点');

  ReactDOM.createRoot(root).render(
    <React.StrictMode>
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </React.StrictMode>,
  );
} catch (e) {
  report('bootstrap', e);
  const root = document.getElementById("root");
  if (root) root.textContent = `启动失败: ${String(e)}`;
}
