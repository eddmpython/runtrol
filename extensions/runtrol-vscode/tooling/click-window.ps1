# Click one client-relative point in one exact titled window. Used by installed-product eye passes where the target
# is a webview and VS Code does not restore its text caret after the Command Palette closes.
param(
    [Parameter(Mandatory = $true)][string]$TitleMatch,
    [Parameter(Mandatory = $true)][int]$X,
    [Parameter(Mandatory = $true)][int]$Y
)

$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class RuntrolClickWin32 {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT point);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
}
"@

$window = Get-Process | Where-Object { $_.MainWindowTitle -like "*$TitleMatch*" } | Select-Object -First 1
if (-not $window) {
    Write-Error "no window has a title matching '$TitleMatch'"
    exit 2
}
$handle = $window.MainWindowHandle
[RuntrolClickWin32]::ShowWindow($handle, 9) | Out-Null
[RuntrolClickWin32]::SetForegroundWindow($handle) | Out-Null
Start-Sleep -Milliseconds 500
if ([RuntrolClickWin32]::GetForegroundWindow() -ne $handle) {
    Write-Error "the window '$($window.MainWindowTitle)' could not be brought to the foreground; nothing was clicked"
    exit 5
}
$rect = New-Object RuntrolClickWin32+RECT
[RuntrolClickWin32]::GetClientRect($handle, [ref]$rect) | Out-Null
if ($X -lt 0 -or $Y -lt 0 -or $X -ge $rect.Right -or $Y -ge $rect.Bottom) {
    Write-Error "the requested client point $X,$Y is outside $($rect.Right)x$($rect.Bottom)"
    exit 3
}
$point = New-Object RuntrolClickWin32+POINT
$point.X = $X
$point.Y = $Y
[RuntrolClickWin32]::ClientToScreen($handle, [ref]$point) | Out-Null
[RuntrolClickWin32]::SetCursorPos($point.X, $point.Y) | Out-Null
# MOUSEEVENTF_LEFTDOWN, then MOUSEEVENTF_LEFTUP.
[RuntrolClickWin32]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
[RuntrolClickWin32]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
Write-Output "clicked client point $X,$Y in '$($window.MainWindowTitle)'"
