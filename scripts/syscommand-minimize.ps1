# 用 WM_SYSCOMMAND/SC_MINIMIZE 最小化窗口，等价于用户点标题栏的最小化按钮。
param([Parameter(Mandatory=$true)][long]$Hwnd)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Sys {
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
}
'@
$h = [IntPtr]$Hwnd
$WM_SYSCOMMAND = 0x0112
$SC_MINIMIZE   = 0xF020
[void][Sys]::SendMessage($h, $WM_SYSCOMMAND, [IntPtr]$SC_MINIMIZE, [IntPtr]::Zero)
Start-Sleep -Milliseconds 800
Write-Output ("minimized={0} visible={1}" -f [Sys]::IsIconic($h), [Sys]::IsWindowVisible($h))
