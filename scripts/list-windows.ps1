# 列出指定进程的所有顶层窗口（含隐藏窗口）。
# 用法: powershell -File list-windows.ps1 -ProcessName paper-pet
param([string]$ProcessName = "paper-pet")

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class WinEnum {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassNameW(IntPtr hWnd, StringBuilder s, int n);
    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);

    public static List<string> ForPid(uint target) {
        var result = new List<string>();
        EnumWindows((h, l) => {
            uint pid;
            GetWindowThreadProcessId(h, out pid);
            if (pid != target) return true;
            var t = new StringBuilder(512);
            GetWindowTextW(h, t, 512);
            var c = new StringBuilder(256);
            GetClassNameW(h, c, 256);
            result.Add(h.ToInt64() + " | visible=" + IsWindowVisible(h) + " | class=" + c + " | title=" + t);
            return true;
        }, IntPtr.Zero);
        return result;
    }
}
'@

$procs = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue
if (-not $procs) { Write-Output "no process: $ProcessName"; exit 1 }

foreach ($p in $procs) {
    Write-Output "=== pid $($p.Id) ==="
    $rows = [WinEnum]::ForPid([uint32]$p.Id)
    if ($rows.Count -eq 0) { Write-Output "  (no top-level windows)" }
    foreach ($r in $rows) { Write-Output "  $r" }
}
