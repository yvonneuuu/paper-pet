# 在指定窗口的中心模拟一次双击（真实鼠标事件，走完整的 WebView2 输入链路）。
# 用途：不靠人手点，验证「双击桌宠打开笔记库」这类交互。
# 用法: powershell -File click-window.ps1 -Hwnd 2361758 -Double
param(
    [Parameter(Mandatory = $true)][long]$Hwnd,
    [switch]$Double,
    [switch]$NoFocus,        # 不预先激活目标窗口，模拟「用户正在别的窗口里工作」
    [int]$OffsetX = 0,
    [int]$OffsetY = 0
)

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class M {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
}
'@ -ReferencedAssemblies System.Drawing

$LEFTDOWN = 0x0002
$LEFTUP   = 0x0004
$TOPMOST  = [IntPtr](-1)
$NOTOPMOST = [IntPtr](-2)
$NOMOVE = 0x0002
$NOSIZE = 0x0001

$h = [IntPtr]$Hwnd
$r = New-Object M+RECT
if (-not [M]::GetWindowRect($h, [ref]$r)) { Write-Output "FAIL GetWindowRect"; exit 1 }

$cx = [int](($r.Left + $r.Right) / 2) + $OffsetX
$cy = [int](($r.Top + $r.Bottom) / 2) + $OffsetY
Write-Output ("target rect {0},{1}-{2},{3}  click at {4},{5}" -f $r.Left, $r.Top, $r.Right, $r.Bottom, $cx, $cy)

# 置顶后再点，避免点到盖在上面的别的窗口
[void][M]::SetWindowPos($h, $TOPMOST, 0, 0, 0, 0, $NOMOVE -bor $NOSIZE)
if (-not $NoFocus) { [void][M]::SetForegroundWindow($h) }
Start-Sleep -Milliseconds 400

# 保存原鼠标位置，测完还回去
Add-Type -AssemblyName System.Windows.Forms
$orig = [System.Windows.Forms.Cursor]::Position

[void][M]::SetCursorPos($cx, $cy)
Start-Sleep -Milliseconds 200

[M]::mouse_event($LEFTDOWN, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 40
[M]::mouse_event($LEFTUP, 0, 0, 0, [IntPtr]::Zero)

if ($Double) {
    Start-Sleep -Milliseconds 90     # 远小于前端 400ms 的双击阈值
    [M]::mouse_event($LEFTDOWN, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 40
    [M]::mouse_event($LEFTUP, 0, 0, 0, [IntPtr]::Zero)
    Write-Output "sent: double click"
} else {
    Write-Output "sent: single click"
}

Start-Sleep -Milliseconds 800
[void][M]::SetCursorPos($orig.X, $orig.Y)
[void][M]::SetWindowPos($h, $NOTOPMOST, 0, 0, 0, 0, $NOMOVE -bor $NOSIZE)
Write-Output "done"
