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
    [string]$CommandLineMatch = ""
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
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int capacity);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
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
function Find-RuntrolWindow([string]$TitleMatch, [string]$CommandLineMatch) {
    $allowedProcessIds = Get-RuntrolProcessFamily $CommandLineMatch
    return [RuntrolWindowWin32]::Visible() | Where-Object {
        $_.Title.IndexOf($TitleMatch, [StringComparison]::OrdinalIgnoreCase) -ge 0 -and
        ($null -eq $allowedProcessIds -or $allowedProcessIds -contains $_.ProcessId)
    } | Select-Object -First 1
}

# Script face: print the matched title so callers can probe for a window's existence.
if ($TitleMatch) {
    $found = Find-RuntrolWindow $TitleMatch $CommandLineMatch
    if ($found) { Write-Output $found.Title }
}
