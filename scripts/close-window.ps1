# 向窗口发送 WM_CLOSE，用来验证应用的关闭行为（等价于点系统标题栏的关闭按钮）。
# 用法: powershell -File close-window.ps1 -Hwnd 330252
param([Parameter(Mandatory = $true)][long]$Hwnd)

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class C {
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
}
'@

$h = [IntPtr]$Hwnd
if (-not [C]::IsWindow($h)) { Write-Output "FAIL not a window: $Hwnd"; exit 1 }

Write-Output "before: visible=$([C]::IsWindowVisible($h))"
[void][C]::SendMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)   # WM_CLOSE
Start-Sleep -Milliseconds 1200

if ([C]::IsWindow($h)) {
    Write-Output "after: still exists, visible=$([C]::IsWindowVisible($h))  (窗口仍然存在)"
} else {
    Write-Output "after: window destroyed  (窗口已销毁)"
}
