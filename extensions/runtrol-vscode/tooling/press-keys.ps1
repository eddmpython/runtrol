# Bring one window, found by its title, to the front and type keys into it. The eye-pass half that proves a
# keyboard path end to end: a window switch reloads the extension host, so nothing inside the test runner
# survives to press the next key; this script does, from outside, exactly as a hand would.
#
# Keys use the SendKeys vocabulary (^ = Ctrl, + = Shift, {ENTER}). Nothing is typed unless the target window is
# verified to be the foreground window first: keys that land in somebody else's window are worse than no keys.
param(
    [Parameter(Mandatory = $true)][string]$TitleMatch,
    [Parameter(Mandatory = $true)][string]$Keys,
    [string]$CommandLineMatch = ""
)

$ErrorActionPreference = "Stop"
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

$allowedProcessIds = $null
if ($CommandLineMatch) {
    $allProcesses = @(Get-CimInstance Win32_Process)
    $allowed = [Collections.Generic.HashSet[int]]::new()
    foreach ($process in $allProcesses) {
        if ($process.CommandLine -and $process.CommandLine.IndexOf($CommandLineMatch, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
            $allowed.Add([int]$process.ProcessId) | Out-Null
        }
    }
    for ($depth = 0; $depth -lt 8; $depth += 1) {
        $before = $allowed.Count
        foreach ($process in $allProcesses) {
            if (-not $allowed.Contains([int]$process.ProcessId)) { continue }
            $parent = $allProcesses | Where-Object { $_.ProcessId -eq $process.ParentProcessId } | Select-Object -First 1
            if ($parent -and $parent.Name -eq "Code.exe") {
                $allowed.Add([int]$parent.ProcessId) | Out-Null
            }
        }
        if ($allowed.Count -eq $before) { break }
    }
    $allowedProcessIds = @($allowed)
}
$window = Get-Process | Where-Object {
    $_.MainWindowTitle -like "*$TitleMatch*" -and
    ($null -eq $allowedProcessIds -or $allowedProcessIds -contains $_.Id)
} | Select-Object -First 1
if (-not $window) {
    Write-Error "no window has a title matching '$TitleMatch'"
    exit 2
}
$handle = $window.MainWindowHandle
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
    Write-Error "the window '$($window.MainWindowTitle)' could not be brought to the foreground; nothing was typed"
    exit 5
}
[System.Windows.Forms.SendKeys]::SendWait($Keys)
Write-Output "pressed '$Keys' in '$($window.MainWindowTitle)'"
