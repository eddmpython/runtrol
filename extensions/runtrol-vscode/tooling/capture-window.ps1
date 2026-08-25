# Photograph one window, found by its title, into a PNG. The eye-pass half that has to live outside the
# extension host: a process cannot photograph its own window from inside the sandboxed test runner.
#
# PrintWindow rather than a screen copy, deliberately: a screen copy photographs whatever pixels happen to be
# on top, which on a busy desktop is somebody else's window. PrintWindow asks the target to render its own
# surface, occluded or not, so the picture is always the window that was asked for. PW_RENDERFULLCONTENT makes
# that work for GPU-composited windows like editors.
param(
    [Parameter(Mandatory = $true)][string]$TitleMatch,
    [Parameter(Mandatory = $true)][string]$OutPath,
    [string]$CommandLineMatch = ""
)

$ErrorActionPreference = "Stop"
# Kept before dot-sourcing: the shared file's own param() block resets these names in this scope.
$wantedTitle = $TitleMatch
$wantedFamily = $CommandLineMatch
. (Join-Path $PSScriptRoot "find-window.ps1")
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class RuntrolCaptureWin32 {
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

$window = Find-RuntrolWindow $wantedTitle $wantedFamily
if (-not $window) {
    Write-Error "no window has a title matching '$wantedTitle'"
    exit 2
}
$handle = [IntPtr]::new([long]$window.Handle)
$rect = New-Object RuntrolCaptureWin32+RECT
[RuntrolCaptureWin32]::GetWindowRect($handle, [ref]$rect) | Out-Null
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -le 0 -or $height -le 0) {
    Write-Error "the window rect is empty ($width x $height)"
    exit 3
}
$bitmap = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$deviceContext = $graphics.GetHdc()
# 2 = PW_RENDERFULLCONTENT, which includes GPU-composited content.
$painted = [RuntrolCaptureWin32]::PrintWindow($handle, $deviceContext, 2)
$graphics.ReleaseHdc($deviceContext)
if (-not $painted) {
    Write-Error "the window refused to render itself"
    exit 4
}
$bitmap.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "captured '$($window.Title)' to $OutPath"
