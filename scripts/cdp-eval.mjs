/**
 * 通过 WebView2 的远程调试端口，在页面里执行一段 JS 并打印结果。
 *
 * 用途：白屏、布局对不上这类问题，靠看代码猜很慢，直接问页面最快。
 *
 * 前置：启动应用时设 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
 * 用法：node scripts/cdp-eval.mjs "<页面标题关键字>" "<要执行的 JS 表达式>" [超时毫秒]
 */
const [, , titleHint = '', expression = '1+1', timeoutMs = '30000'] = process.argv;

const targets = await fetch('http://127.0.0.1:9222/json').then((r) => r.json());
const pages = targets.filter((t) => t.type === 'page');

if (pages.length === 0) {
  console.error('没有找到任何页面。应用起来了吗？调试端口开了吗？');
  process.exit(1);
}

console.error(
  '可用页面: ' + pages.map((p) => `${p.title} (${p.url})`).join(' | '),
);

const target =
  pages.find((p) => p.title.includes(titleHint) || p.url.includes(titleHint)) ??
  pages[0];
console.error(`选中: ${target.title}`);

const ws = new WebSocket(target.webSocketDebuggerUrl);

const result = await new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error('超时')), Number(timeoutMs));

  ws.onopen = () => {
    ws.send(
      JSON.stringify({
        id: 1,
        method: 'Runtime.evaluate',
        params: { expression, returnByValue: true, awaitPromise: true },
      }),
    );
  };

  ws.onmessage = (e) => {
    const msg = JSON.parse(e.data);
    if (msg.id === 1) {
      clearTimeout(timer);
      ws.close();
      if (msg.result?.exceptionDetails) {
        reject(new Error(JSON.stringify(msg.result.exceptionDetails, null, 2)));
      } else {
        resolve(msg.result?.result?.value);
      }
    }
  };

  ws.onerror = (e) => {
    clearTimeout(timer);
    reject(new Error('WebSocket 错误: ' + e.message));
  };
});

console.log(typeof result === 'string' ? result : JSON.stringify(result, null, 2));
