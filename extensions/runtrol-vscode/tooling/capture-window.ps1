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
$rect = New-Object RuntrolCaptureWin32+RECT
[RuntrolCaptureWin32]::GetWindowRect($window.MainWindowHandle, [ref]$rect) | Out-Null
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
$painted = [RuntrolCaptureWin32]::PrintWindow($window.MainWindowHandle, $deviceContext, 2)
$graphics.ReleaseHdc($deviceContext)
if (-not $painted) {
    Write-Error "the window refused to render itself"
    exit 4
}
$bitmap.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "captured '$($window.MainWindowTitle)' to $OutPath"
