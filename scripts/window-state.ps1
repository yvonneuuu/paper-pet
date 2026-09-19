# 打印窗口状态：是否最小化 / 是否可见。
param([Parameter(Mandatory=$true)][long]$Hwnd)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class St {
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
}
'@
$h = [IntPtr]$Hwnd
Write-Output ("exists={0} minimized={1} visible={2}" -f [St]::IsWindow($h), [St]::IsIconic($h), [St]::IsWindowVisible($h))
