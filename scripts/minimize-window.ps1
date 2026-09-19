# 最小化指定窗口（用于复现"最小化后叫不回来"）。
param([Parameter(Mandatory=$true)][long]$Hwnd)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Mz {
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
}
'@
$h = [IntPtr]$Hwnd
[void][Mz]::ShowWindow($h, 6)   # SW_MINIMIZE
Start-Sleep -Milliseconds 600
Write-Output ("minimized={0} visible={1}" -f [Mz]::IsIconic($h), [Mz]::IsWindowVisible($h))
