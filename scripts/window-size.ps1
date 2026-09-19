# 输出窗口面积（像素数），用于在不依赖标题的情况下区分桌宠窗口和笔记库窗口。
# 用法: powershell -File window-size.ps1 -Hwnd 123456
param([Parameter(Mandatory = $true)][long]$Hwnd)

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class S {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
}
'@

$r = New-Object S+RECT
if ([S]::GetWindowRect([IntPtr]$Hwnd, [ref]$r)) {
    Write-Output (($r.Right - $r.Left) * ($r.Bottom - $r.Top))
} else {
    Write-Output 0
}
