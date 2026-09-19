#!/usr/bin/env bash
# 端到端冒烟测试：不靠人手点，把 P0 的关键交互全跑一遍。
#
# 覆盖（对应 05-演示脚本.md §四 演示前检查表）：
#   1. 应用启动、两个窗口都建出来
#   2. 双击桌宠 → 打开笔记库
#   3. 点笔记卡片 → 引用面板
#   4. 关闭笔记库 → 只隐藏，进程存活
#   5. 关闭桌宠 → 整个应用退出
#   6. 剪贴板复制 → 入库 + 抓到窗口标题
#
# 用法: bash scripts/smoke-test.sh
set -u

cd "$(dirname "$0")/.."
PS="powershell -ExecutionPolicy Bypass -File"
SHOT_DIR="${TEMP:-/tmp}"
LOG="${TEMP:-/tmp}/paper-pet-smoke.log"

pass=0
fail=0

ok()   { echo "  [PASS] $1"; pass=$((pass+1)); }
bad()  { echo "  [FAIL] $1"; fail=$((fail+1)); }

windows() {
  $PS scripts/list-windows.ps1 -ProcessName paper-pet 2>/dev/null | grep -a "Tauri Window"
}

# 取「可见 / 不可见」的 Tauri 窗口句柄。桌宠始终在，笔记库按状态变。
hwnd_of() {  # $1 = pet|library
  local rows; rows=$(windows)
  if [ "$1" = "pet" ]; then
    # 桌宠是 200x200 的那个，比笔记库小得多
    echo "$rows" | while IFS='|' read -r h rest; do
      local hh; hh=$(echo "$h" | tr -d ' ')
      local size; size=$($PS scripts/window-size.ps1 -Hwnd "$hh" 2>/dev/null | tr -d '\r')
      if [ "${size:-0}" -lt 200000 ]; then echo "$hh"; return; fi
    done
  else
    echo "$rows" | while IFS='|' read -r h rest; do
      local hh; hh=$(echo "$h" | tr -d ' ')
      local size; size=$($PS scripts/window-size.ps1 -Hwnd "$hh" 2>/dev/null | tr -d '\r')
      if [ "${size:-0}" -ge 200000 ]; then echo "$hh"; return; fi
    done
  fi
}

echo "=== 0. 启动应用 ==="
powershell -Command "Get-Process paper-pet,node -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue" >/dev/null 2>&1
sleep 3
rm -f "$LOG"
# 开远程调试端口，第 2 步要用 CDP 驱动界面
export WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
(npm run tauri dev > "$LOG" 2>&1 &)
until grep -q "paper-pet.exe" "$LOG" 2>/dev/null; do :; done
sleep 6

rows=$(windows)
n=$(echo "$rows" | grep -c "Tauri Window")
if [ "$n" -eq 2 ]; then ok "两个窗口都已创建"; else bad "期望 2 个 Tauri 窗口，实际 $n"; fi

PET=$(hwnd_of pet)
LIB=$(hwnd_of library)
echo "  pet=$PET library=$LIB"

if echo "$rows" | grep -q "$LIB | visible=False"; then
  ok "笔记库启动时隐藏（visible=False）"
else
  bad "笔记库启动时不该可见"
fi

echo
echo "=== 1. 双击桌宠打开笔记库 ==="
$PS scripts/click-window.ps1 -Hwnd "$PET" -Double >/dev/null 2>&1
sleep 2
if windows | grep -q "$LIB | visible=True"; then
  ok "笔记库已显示"
else
  bad "双击后笔记库仍未显示"
fi

echo
echo "=== 2. 打开一篇文章，看证据卡与出处核对面板 ==="
# 通过 CDP 驱动，比盲点坐标可靠（新界面是文章列表，不是笔记列表）
if node scripts/cdp-eval.mjs "localhost:1420" "
(async () => {
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const card = document.querySelector('.sticky');
  if (!card) return 'NO_PAPER';
  card.click(); await sleep(2000);
  const n = document.querySelectorAll('.excerpt').length;
  const panel = !!document.querySelector('.panel-head');
  return (n > 0 && panel) ? 'OK:' + n : 'FAIL';
})()
" 30000 2>/dev/null | grep -q "^OK:"; then
  ok "文章页渲染出证据卡 + 出处核对面板"
else
  bad "文章页渲染异常（或库为空）"
fi
$PS scripts/show-window.ps1 -Hwnd "$LIB" -Shot "$SHOT_DIR/smoke-citation.png" >/dev/null 2>&1
[ -f "$SHOT_DIR/smoke-citation.png" ] && ok "已截图: $SHOT_DIR/smoke-citation.png" || bad "截图失败"

echo
echo "=== 3. 关闭笔记库（应只隐藏，进程存活）==="
$PS scripts/close-window.ps1 -Hwnd "$LIB" >/dev/null 2>&1
sleep 2
if windows | grep -q "$LIB | visible=False"; then
  ok "笔记库已隐藏"
else
  bad "笔记库关闭后仍可见"
fi
alive=$(powershell -Command "Get-Process paper-pet -EA SilentlyContinue | Measure-Object | Select-Object -ExpandProperty Count" 2>/dev/null | tr -d '\r')
if [ "${alive:-0}" -eq 1 ]; then ok "进程存活"; else bad "进程数=$alive（应为 1）"; fi

echo
echo "=== 4. 剪贴板捕获 ==="
before=$(node -e "const{DatabaseSync}=require('node:sqlite');const d=new DatabaseSync(process.env.APPDATA+'/com.yiiiii.paper-pet/notes.db');console.log(d.prepare('SELECT COUNT(*) n FROM notes').get().n)" 2>/dev/null)
powershell -Command "Set-Clipboard -Value 'smoke-test-$(date +%s) 测试内容[9]'" >/dev/null 2>&1
sleep 2
after=$(node -e "const{DatabaseSync}=require('node:sqlite');const d=new DatabaseSync(process.env.APPDATA+'/com.yiiiii.paper-pet/notes.db');console.log(d.prepare('SELECT COUNT(*) n FROM notes').get().n)" 2>/dev/null)
if [ "${after:-0}" -gt "${before:-0}" ]; then
  ok "复制后笔记 +1（$before → $after）"
  node -e "const{DatabaseSync}=require('node:sqlite');const d=new DatabaseSync(process.env.APPDATA+'/com.yiiiii.paper-pet/notes.db');const r=d.prepare('SELECT source_title,source_type FROM notes ORDER BY id DESC LIMIT 1').get();console.log('  来源:',JSON.stringify(r))" 2>/dev/null
else
  bad "复制后笔记数没变（$before → $after）"
fi

echo
echo "=== 5. 关闭桌宠（应退出整个应用）==="
$PS scripts/close-window.ps1 -Hwnd "$PET" >/dev/null 2>&1
sleep 3
alive=$(powershell -Command "Get-Process paper-pet -EA SilentlyContinue | Measure-Object | Select-Object -ExpandProperty Count" 2>/dev/null | tr -d '\r')
if [ "${alive:-1}" -eq 0 ]; then ok "应用已退出"; else bad "关闭桌宠后进程仍在（$alive）"; fi

echo
echo "=== 后端日志 ==="
grep -a "paper-pet\]" "$LOG" | tail -10

echo
echo "=================================="
echo "  PASS: $pass   FAIL: $fail"
echo "=================================="
[ "$fail" -eq 0 ]
