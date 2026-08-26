# Click one client-relative point in one exact titled window. Used by installed-product eye passes where the target
# is a webview and VS Code does not restore its text caret after the Command Palette closes.
param(
    [Parameter(Mandatory = $true)][string]$TitleMatch,
    [Parameter(Mandatory = $true)][int]$X,
    [Parameter(Mandatory = $true)][int]$Y,
    # Right for a context menu. The reorder and rename actions live there, which is where a list's own
    # arrangement belongs rather than as more buttons on every row.
    [ValidateSet("left", "right")][string]$Button = "left",
    # Two windows can carry the same title when they hold the same folder, and the operator's window is one of
    # them. The process family (a user-data-dir, say) is what tells an isolated window from theirs.
    [string]$CommandLineMatch = ""
)

$ErrorActionPreference = "Stop"
# Kept before dot-sourcing: the shared file's own param() block resets these names in this scope.
$wantedTitle = $TitleMatch
$wantedFamily = $CommandLineMatch
. (Join-Path $PSScriptRoot "find-window.ps1")
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

$window = Find-RuntrolWindow $wantedTitle $wantedFamily
if (-not $window) {
    Write-Error "no window has a title matching '$wantedTitle'"
    exit 2
}
$handle = [IntPtr]::new([long]$window.Handle)
if (-not (Set-RuntrolWindowFocus $window.Handle)) {
    Write-Error "the window '$($window.Title)' could not be brought to the foreground; nothing was clicked"
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
# A move, not a jump. SetCursorPos to where the pointer already is produces no mouse-move at all, so a second
# press on the same control arrives with no hover event in front of it and the editor ignores it (measured
# 2026-08-26: pressing the usage section's plus twice opened the list and then did nothing).
[RuntrolClickWin32]::SetCursorPos($point.X + 40, $point.Y + 40) | Out-Null
Start-Sleep -Milliseconds 120
[RuntrolClickWin32]::SetCursorPos($point.X, $point.Y) | Out-Null
# The editor reveals a view's title actions on hover, and the renderer needs a frame to notice the pointer
# arrived. Pressing in the same instant lands on whatever was drawn before the hover, so the press appears to
# do nothing while the tool reports success (measured 2026-08-26: the usage section's plus never fired).
Start-Sleep -Milliseconds 400
# MOUSEEVENTF_LEFTDOWN/UP, or RIGHTDOWN/UP.
$down = if ($Button -eq "right") { 0x0008 } else { 0x0002 }
$up = if ($Button -eq "right") { 0x0010 } else { 0x0004 }
[RuntrolClickWin32]::mouse_event($down, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 80
[RuntrolClickWin32]::mouse_event($up, 0, 0, 0, [UIntPtr]::Zero)
Write-Output "$Button-clicked client point $X,$Y in '$($window.Title)'"
