# 把隐藏/后台的窗口显示出来、挪到左上角并截图。
# 用途：在没有人手动点的情况下，验证某个窗口实际渲染出了什么。
#
# 用法:
#   powershell -File show-window.ps1 -Title "论文笔记库" -Shot out.png
#   powershell -File show-window.ps1 -ProcessName paper-pet -Shot out.png   # 取该进程第一个 Tauri 窗口
param(
    [string]$Title = "",
    [string]$ProcessName = "",
    [long]$Hwnd = 0,
    [string]$Shot = "",
    [int]$Width = 1100,
    [int]$Height = 800
)

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class W {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    public delegate bool EnumProc(IntPtr h, IntPtr l);

    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint f);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);

    public static IntPtr FindByTitle(string title) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            var sb = new StringBuilder(512);
            GetWindowTextW(h, sb, 512);
            if (sb.ToString() == title) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static List<string> TauriWindowsOf(uint pid) {
        var res = new List<string>();
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p != pid) return true;
            var cls = new StringBuilder(256); GetClassNameW(h, cls, 256);
            var txt = new StringBuilder(512); GetWindowTextW(h, txt, 512);
            res.Add(h.ToInt64() + "\t" + cls + "\t" + txt);
            return true;
        }, IntPtr.Zero);
        return res;
    }
}
'@

$SWP_NOZORDER = 0x0004
$SWP_SHOWWINDOW = 0x0040

# ---- 解析目标窗口 ----
if ($Hwnd -ne 0) {
    $h = [IntPtr]$Hwnd
} elseif ($Title -ne "") {
    $h = [W]::FindByTitle($Title)
} elseif ($ProcessName -ne "") {
    $procs = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue
    if (-not $procs) { Write-Output "FAIL no process $ProcessName"; exit 1 }
    # 按面积挑最大的那个 Tauri 窗口。
    # 不用标题匹配：本文件是 UTF-8，Windows PowerShell 5.1 按 ANSI 读，
    # 里面的中文字面量会变成乱码，比较必然失败。
    $h = [IntPtr]::Zero
    $best = -1
    foreach ($row in [W]::TauriWindowsOf([uint32]$procs[0].Id)) {
        $parts = $row -split "`t"
        if ($parts[1] -ne "Tauri Window") { continue }
        $cand = [IntPtr][long]$parts[0]
        $rr = New-Object W+RECT
        [void][W]::GetWindowRect($cand, [ref]$rr)
        $area = ($rr.Right - $rr.Left) * ($rr.Bottom - $rr.Top)
        if ($area -gt $best) { $best = $area; $h = $cand }
    }
} else {
    Write-Output "FAIL 需要 -Title / -Hwnd / -ProcessName 之一"
    exit 1
}

if ($h -eq [IntPtr]::Zero) { Write-Output "FAIL window not found"; exit 1 }
Write-Output "hwnd=$h visible=$([W]::IsWindowVisible($h))"

# ---- 显示并挪到左上角 ----
[void][W]::ShowWindow($h, 9)   # SW_RESTORE
[void][W]::ShowWindow($h, 5)   # SW_SHOW
[void][W]::SetWindowPos($h, [IntPtr]::Zero, 0, 0, $Width, $Height, $SWP_NOZORDER -bor $SWP_SHOWWINDOW)

if ($Shot -eq "") { Write-Output "shown (no screenshot requested)"; exit 0 }

Start-Sleep -Milliseconds 2500

$r = New-Object W+RECT
[void][W]::GetWindowRect($h, [ref]$r)
$w = $r.Right - $r.Left
$ht = $r.Bottom - $r.Top
Write-Output ("rect: {0},{1} {2}x{3}" -f $r.Left, $r.Top, $w, $ht)
if ($w -le 0 -or $ht -le 0) { Write-Output "FAIL bad rect"; exit 1 }

Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
$bmp.Save($Shot, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
Write-Output "saved: $Shot"
