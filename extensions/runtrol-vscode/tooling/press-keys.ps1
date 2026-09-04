# Bring one window, found by its title, to the front and type keys into it. The eye-pass half that proves a
# keyboard path end to end: a window switch reloads the extension host, so nothing inside the test runner
# survives to press the next key; this script does, from outside, exactly as a hand would.
#
# Keys use the SendKeys vocabulary (^ = Ctrl, + = Shift, {ENTER}). Nothing is typed unless the target window is
# verified to be the foreground window first: keys that land in somebody else's window are worse than no keys.
param(
    [string]$TitleMatch = "",
    [Parameter(Mandatory = $true)][string]$Keys,
    [string]$CommandLineMatch = "",
    [int]$ProcessId = 0
)

$ErrorActionPreference = "Stop"
# Kept before dot-sourcing: the shared file's own param() block resets these names in this scope.
$wantedTitle = $TitleMatch
$wantedFamily = $CommandLineMatch
$wantedProcess = $ProcessId
. (Join-Path $PSScriptRoot "find-window.ps1")
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class RuntrolPressWin32 {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, IntPtr lpdwProcessId);
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
}
"@

$window = Find-RuntrolWindow $wantedTitle $wantedFamily $wantedProcess
if (-not $window) {
    Write-Error "no window has a title matching '$wantedTitle' (process $wantedProcess)"
    exit 2
}
$handle = [IntPtr]::new([long]$window.Handle)
# 9 = SW_RESTORE: a minimised window takes no keys.
[RuntrolPressWin32]::ShowWindow($handle, 9) | Out-Null

# Windows lets a background process take the foreground only when it is attached to the thread that has it,
# so the attempt attaches, asks, and checks; three tries before giving up without typing.
$focused = $false
for ($attempt = 0; $attempt -lt 3 -and -not $focused; $attempt += 1) {
    $foreground = [RuntrolPressWin32]::GetForegroundWindow()
    $foregroundThread = [RuntrolPressWin32]::GetWindowThreadProcessId($foreground, [IntPtr]::Zero)
    $targetThread = [RuntrolPressWin32]::GetWindowThreadProcessId($handle, [IntPtr]::Zero)
    $ownThread = [RuntrolPressWin32]::GetCurrentThreadId()
    [RuntrolPressWin32]::AttachThreadInput($ownThread, $foregroundThread, $true) | Out-Null
    [RuntrolPressWin32]::AttachThreadInput($ownThread, $targetThread, $true) | Out-Null
    [RuntrolPressWin32]::BringWindowToTop($handle) | Out-Null
    [RuntrolPressWin32]::SetForegroundWindow($handle) | Out-Null
    [RuntrolPressWin32]::AttachThreadInput($ownThread, $targetThread, $false) | Out-Null
    [RuntrolPressWin32]::AttachThreadInput($ownThread, $foregroundThread, $false) | Out-Null
    Start-Sleep -Milliseconds 500
    $focused = ([RuntrolPressWin32]::GetForegroundWindow() -eq $handle)
}
if (-not $focused) {
    Write-Error "the window '$($window.Title)' could not be brought to the foreground; nothing was typed"
    exit 5
}
[System.Windows.Forms.SendKeys]::SendWait($Keys)
Write-Output "pressed '$Keys' in '$($window.Title)'"
