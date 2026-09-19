# 截屏。默认截全屏；给 -Hwnd 则只截该窗口所在的矩形（不移动、不改变窗口）。
# 用法:
#   powershell -File screenshot.ps1 -Out shot.png
#   powershell -File screenshot.ps1 -Hwnd 2950424 -Out pet.png -Pad 40
param(
    [string]$Out = "screenshot.png",
    [long]$Hwnd = 0,
    [int]$Pad = 0
)

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Snap {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
}
'@

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

if ($Hwnd -ne 0) {
    $r = New-Object Snap+RECT
    if (-not [Snap]::GetWindowRect([IntPtr]$Hwnd, [ref]$r)) {
        Write-Output "FAIL GetWindowRect"; exit 1
    }
    $x = $r.Left - $Pad
    $y = $r.Top - $Pad
    $w = ($r.Right - $r.Left) + $Pad * 2
    $h = ($r.Bottom - $r.Top) + $Pad * 2
} else {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $x = $b.X; $y = $b.Y; $w = $b.Width; $h = $b.Height
}

if ($w -le 0 -or $h -le 0) { Write-Output "FAIL bad size"; exit 1 }

$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output ("saved: {0}  region {1},{2} {3}x{4}" -f $Out, $x, $y, $w, $h)
