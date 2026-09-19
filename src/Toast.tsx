/**
 * 通用提示（02 §3.5）+ 撤销入口（09 §2.4）。
 *
 * 顶部短暂浮出。带「撤销」的那条停 6 秒——2 秒不够读完再决定要不要点，
 * 而撤销恰恰是需要读一下才敢按的操作。
 */
import { useCallback, useEffect, useRef, useState } from 'react';

export type ToastTone = 'error' | 'success';

/** 09 §2.4：Toast 右侧那个按钮。目前只有「撤销」用它。 */
export interface ToastAction {
  label: string;
  run: () => void;
}

interface ToastItem {
  id: number;
  text: string;
  tone: ToastTone;
  action?: ToastAction;
}

const DURATION_MS = 2000;
const ACTION_DURATION_MS = 6000;

export function useToast() {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const seq = useRef(0);
  const timers = useRef<number[]>([]);

  const dismiss = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const show = useCallback(
    (text: string, tone: ToastTone = 'error', action?: ToastAction) => {
      const id = ++seq.current;
      setToasts((prev) => [...prev, { id, text, tone, action }]);

      const timer = window.setTimeout(
        () => setToasts((prev) => prev.filter((t) => t.id !== id)),
        action ? ACTION_DURATION_MS : DURATION_MS,
      );
      timers.current.push(timer);
    },
    [],
  );

  // 组件卸载时清掉未触发的定时器，避免对已卸载组件 setState
  useEffect(
    () => () => {
      timers.current.forEach(window.clearTimeout);
      timers.current = [];
    },
    [],
  );

  return { toasts, show, dismiss };
}

export function ToastHost({
  toasts,
  onDismiss,
}: {
  toasts: ToastItem[];
  onDismiss?: (id: number) => void;
}) {
  if (toasts.length === 0) return null;

  return (
    <div className="toast-host">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.tone}`}>
          <span className="toast-text">{t.text}</span>
          {t.action && (
            <button
              className="toast-action"
              onClick={() => {
                // 先收起来再执行：撤销本身会弹新的 Toast，
                // 不收的话两条会叠在一起，看着像没反应
                onDismiss?.(t.id);
                t.action?.run();
              }}
            >
              {t.action.label}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

/** 后端 `Err(String)` 之外还可能抛出别的东西，统一转成可展示的文案。 */
export function errorText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  if (e && typeof e === 'object' && 'message' in e && typeof (e as { message: unknown }).message === 'string') {
    return (e as { message: string }).message;
  }
  return '操作失败';
}
