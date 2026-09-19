import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { LibraryView } from "./LibraryView";
import "./App.css";

type PetState = 'idle' | 'receiving' | 'success';

/** 02 §2.5 的阈值：位移 > 5px 算拖拽；两次按下间隔 < 500ms 算双击。 */
const DRAG_THRESHOLD_PX = 5;
/** 对齐 Windows 的系统默认双击间隔（GetDoubleClickTime 默认 500ms）。
 *  原来定的 400ms 比系统还严，真人容易点不出来。 */
const DOUBLE_CLICK_MS = 500;

// ---------- 桌宠视图（main 窗口，02 §2） ----------

function PetView() {
  const [petState, setPetState] = useState<PetState>('idle');
  const [noteCount, setNoteCount] = useState(0);

  // 区分「拖动」和「双击」：按下时记录坐标，移动超过阈值才算拖（02 §2.5）
  const pressPos = useRef<{ x: number; y: number } | null>(null);
  const isDraggingRef = useRef(false);
  const lastDownAt = useRef(0);
  const timers = useRef<number[]>([]);

  async function openLibrary() {
    try {
      await invoke('open_library');
    } catch (e) {
      // 桌宠窗口没有 Toast 的位置，把失败送回后端日志，别让它悄悄消失
      invoke('log_frontend_error', {
        message: `open_library 失败: ${String(e)}`,
      }).catch(() => {});
    }
  }

  /**
   * 双击判定放在 pointerdown，不放在 pointerup（02 §2.5）。
   *
   * 放 pointerup 会有个很难查的失效：真人双击时手常抖过 5px 阈值，
   * 第一次点击就触发了 `startDragging()`，窗口进入系统拖拽模式后
   * **webview 收不到那次的 pointerup**——于是第二次点击被当成「第一次」，
   * 双击永远凑不齐。判定挪到按下，中间有没有误触发拖拽都不影响。
   */
  function handlePointerDown(e: React.PointerEvent) {
    if (e.button !== 0) return;

    const now = Date.now();
    if (now - lastDownAt.current < DOUBLE_CLICK_MS) {
      lastDownAt.current = 0; // 消费掉，避免三连击触发两次
      pressPos.current = null;
      isDraggingRef.current = false;
      openLibrary();
      return;
    }

    lastDownAt.current = now;
    pressPos.current = { x: e.clientX, y: e.clientY };
    isDraggingRef.current = false;
  }

  function handlePointerMove(e: React.PointerEvent) {
    if (!pressPos.current) return;
    const dx = Math.abs(e.clientX - pressPos.current.x);
    const dy = Math.abs(e.clientY - pressPos.current.y);

    if (dx > DRAG_THRESHOLD_PX || dy > DRAG_THRESHOLD_PX) {
      if (!isDraggingRef.current) {
        isDraggingRef.current = true;
        // 只调一次：拖拽中持续调用 startDragging 会让窗口抖动
        getCurrentWindow().startDragging();
      }
    }
  }

  function handlePointerUp() {
    // 双击已经在 pointerdown 判完了，这里只负责收尾。
    // 注意：拖拽期间这个回调可能根本不会被调用（事件被系统拿走了）。
    pressPos.current = null;
    isDraggingRef.current = false;
  }

  useEffect(() => {
    // 角标数 = 摘录总数
    invoke<unknown[]>('get_notes')
      .then((notes) => setNoteCount(notes.length))
      .catch(() => {
        /* 初始读数失败不打扰用户，角标维持 0 */
      });

    const unlisten = listen<string>('clipboard-update', () => {
      // 01 §4 状态机：0ms receiving → 500ms success → 2000ms idle
      setPetState('receiving');
      setNoteCount((prev) => prev + 1);

      timers.current.push(window.setTimeout(() => setPetState('success'), 500));
      timers.current.push(window.setTimeout(() => setPetState('idle'), 2000));
    });

    return () => {
      unlisten.then((fn) => fn());
      timers.current.forEach(window.clearTimeout);
      timers.current = [];
    };
  }, []);

  return (
    <div
      className="pet-container"
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
    >
      <img
        src={`/pet-${petState}.png`}
        alt="Paper Pet"
        className="pet-image"
        draggable={false}
      />
      {noteCount > 0 && (
        <div className="note-badge">{noteCount > 99 ? '99+' : noteCount}</div>
      )}
      <button
        className="pet-close"
        title="关闭"
        // 避免触发拖拽/双击逻辑（02 §2.4）
        onPointerDown={(e) => e.stopPropagation()}
        onClick={() => getCurrentWindow().close()}
      >
        ✕
      </button>
    </div>
  );
}

export default function App() {
  // 01 §5.4 / 02 §1：只认窗口 label，不用 URL query 参数
  const isLibrary = getCurrentWindow().label === "library";

  // 桌宠窗口要让透明区域鼠标穿透，笔记库窗口绝对不能——
  // 两个窗口跑的是同一份 CSS，用根节点属性区分，避免互相污染。
  document.documentElement.dataset.view = isLibrary ? 'library' : 'pet';

  return isLibrary ? <LibraryView /> : <PetView />;
}
