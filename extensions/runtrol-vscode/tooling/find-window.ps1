# Find one top-level window by a title substring, optionally confined to one process family. The one lookup
# behind capture-window.ps1 and press-keys.ps1 (dot-sourced) and behind the existence probes in
# inspect-vscode.mjs and real-window-eye.mjs (run as a script: prints the matched title, or nothing).
#
# The family walk exists because folder switches can replace the original root process, and that successor may
# omit the user-data argument even though its renderer children retain it.
#
# Top-level windows are enumerated with EnumWindows rather than read off Get-Process: VS Code opens every one
# of its windows from a single main process, and Get-Process reports only the focused one of them as
# MainWindowTitle, which hid the operator's second window from the eye entirely (measured 2026-08-25).
#
# The title is a plain substring, never a wildcard or regex: window titles carry brackets and dots that would
# otherwise turn a literal request into a pattern.
param(
    # Not mandatory: dot-sourcing passes nothing, and a mandatory parameter would prompt.
    [string]$TitleMatch = "",
    [string]$CommandLineMatch = "",
    # The exact process that owns the window, when the caller knows it: a console window belongs to its shell and
    # a Windows Terminal window to WindowsTerminal.exe (measured 2026-09-05), and a provider retitles both.
    [int]$ProcessId = 0
)

$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class RuntrolWindowWin32 {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int capacity);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", EntryPoint = "GetWindowThreadProcessId")] public static extern uint GetWindowThreadProcessIdOnly(IntPtr hWnd, IntPtr lpdwProcessId);
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
    public class TopLevelWindow {
        public long Handle { get; set; }
        public string Title { get; set; }
        public int ProcessId { get; set; }
    }
    public static List<TopLevelWindow> Visible() {
        var found = new List<TopLevelWindow>();
        EnumWindows((hWnd, lParam) => {
            if (!IsWindowVisible(hWnd)) return true;
            var text = new StringBuilder(1024);
            GetWindowText(hWnd, text, text.Capacity);
            if (text.Length == 0) return true;
            uint processId;
            GetWindowThreadProcessId(hWnd, out processId);
            found.Add(new TopLevelWindow { Handle = hWnd.ToInt64(), Title = text.ToString(), ProcessId = (int)processId });
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
"@

# The process ids of one VS Code family: every process whose command line carries the marker (an isolated
# user-data-dir, typically) and, walking up, the Code.exe parents that own them. $null means "any process".
# Bring one window to the foreground and say whether it worked.
#
# Windows lets a background process take the foreground only while it is attached to the thread that already
# has it, so this attaches, asks, detaches, and checks. Three tries before giving up. One copy, because a tool
# that only asks once fails silently and looks like a click that did nothing (measured 2026-08-26).
function Set-RuntrolWindowFocus([long]$Handle) {
    $target = [IntPtr]::new($Handle)
    # 9 = SW_RESTORE: a minimised window takes neither keys nor clicks.
    [RuntrolWindowWin32]::ShowWindow($target, 9) | Out-Null
    for ($attempt = 0; $attempt -lt 3; $attempt += 1) {
        $foreground = [RuntrolWindowWin32]::GetForegroundWindow()
        $foregroundThread = [RuntrolWindowWin32]::GetWindowThreadProcessIdOnly($foreground, [IntPtr]::Zero)
        $targetThread = [RuntrolWindowWin32]::GetWindowThreadProcessIdOnly($target, [IntPtr]::Zero)
        $ownThread = [RuntrolWindowWin32]::GetCurrentThreadId()
        [RuntrolWindowWin32]::AttachThreadInput($ownThread, $foregroundThread, $true) | Out-Null
        [RuntrolWindowWin32]::AttachThreadInput($ownThread, $targetThread, $true) | Out-Null
        [RuntrolWindowWin32]::BringWindowToTop($target) | Out-Null
        [RuntrolWindowWin32]::SetForegroundWindow($target) | Out-Null
        [RuntrolWindowWin32]::AttachThreadInput($ownThread, $targetThread, $false) | Out-Null
        [RuntrolWindowWin32]::AttachThreadInput($ownThread, $foregroundThread, $false) | Out-Null
        Start-Sleep -Milliseconds 500
        if ([RuntrolWindowWin32]::GetForegroundWindow() -eq $target) { return $true }
    }
    return $false
}

function Get-RuntrolProcessFamily([string]$CommandLineMatch) {
    if (-not $CommandLineMatch) { return $null }
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
    return @($allowed)
}

# The first visible top-level window whose title contains $TitleMatch (case-insensitive), within the family
# when one is named. Returns $null when nothing matches; the caller decides what that means.
function Find-RuntrolWindow([string]$TitleMatch, [string]$CommandLineMatch, [int]$ProcessId = 0) {
    # Never "any window": a lookup with neither a title nor a process would hand back whatever is on top, and keys
    # typed into that window are keys typed into the operator's work (measured 2026-09-05, once).
    if (-not $TitleMatch -and $ProcessId -le 0) { return $null }
    $allowedProcessIds = Get-RuntrolProcessFamily $CommandLineMatch
    return [RuntrolWindowWin32]::Visible() | Where-Object {
        ((($ProcessId -gt 0) -and ($_.ProcessId -eq $ProcessId)) -or
         (($ProcessId -le 0) -and ($_.Title.IndexOf($TitleMatch, [StringComparison]::OrdinalIgnoreCase) -ge 0))) -and
        ($null -eq $allowedProcessIds -or $allowedProcessIds -contains $_.ProcessId)
    } | Select-Object -First 1
}

# Script face: print the matched title so callers can probe for a window's existence.
if ($TitleMatch -or $ProcessId -gt 0) {
    $found = Find-RuntrolWindow $TitleMatch $CommandLineMatch $ProcessId
    if ($found) { Write-Output $found.Title }
}
