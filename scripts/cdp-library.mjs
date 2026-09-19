/**
 * 在**笔记库那个页面**里执行 JS。
 *
 * 为什么不能用 cdp-eval.mjs：桌宠和笔记库是同一份前端、同一个 URL，
 * `document.title` 也一样，按标题根本挑不出来。这里改成问页面自己
 * （`html[data-view]`，App.tsx 启动时写的），才能确定选中的是哪一个。
 *
 * 用法: node scripts/cdp-library.mjs "<要执行的 JS 表达式>" [超时毫秒]
 */
const [, , expression = '1+1', timeoutMs = '30000'] = process.argv;

const targets = await fetch('http://127.0.0.1:9222/json').then((r) => r.json());
const pages = targets.filter((t) => t.type === 'page');

if (pages.length === 0) {
  console.error('没有找到任何页面。应用起来了吗？调试端口开了吗？');
  process.exit(1);
}

async function evaluate(target, expr) {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  try {
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('超时')), Number(timeoutMs));
      ws.onopen = () =>
        ws.send(
          JSON.stringify({
            id: 1,
            method: 'Runtime.evaluate',
            params: { expression: expr, returnByValue: true, awaitPromise: true },
          }),
        );
      ws.onmessage = (e) => {
        const msg = JSON.parse(e.data);
        if (msg.id !== 1) return;
        clearTimeout(timer);
        ws.close();
        if (msg.result?.exceptionDetails) {
          reject(new Error(JSON.stringify(msg.result.exceptionDetails.exception ?? msg.result.exceptionDetails)));
        } else {
          resolve(msg.result?.result?.value);
        }
      };
      ws.onerror = () => {
        clearTimeout(timer);
        reject(new Error('WebSocket 错误'));
      };
    });
  } finally {
    try {
      ws.close();
    } catch {
      /* 已经关了 */
    }
  }
}

let library = null;
for (const p of pages) {
  const view = await evaluate(p, 'document.documentElement.dataset.view');
  console.error(`页面 ${p.url} → data-view=${view}`);
  if (view === 'library') library = p;
}

if (!library) {
  console.error('没找到笔记库页面（data-view=library）');
  process.exit(1);
}

const out = await evaluate(library, expression);
console.log(typeof out === 'string' ? out : JSON.stringify(out, null, 2));
