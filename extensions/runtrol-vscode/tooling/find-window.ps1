# Find one VS Code window inside an isolated process family. Folder switches can replace the original root
# process, and that successor may omit the user-data argument even though its renderer children retain it.
param(
    [Parameter(Mandatory = $true)][string]$TitleMatch,
    [string]$CommandLineMatch = ""
)

$ErrorActionPreference = "Stop"
$allProcesses = @(Get-CimInstance Win32_Process)
$allowed = [Collections.Generic.HashSet[int]]::new()
if ($CommandLineMatch) {
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
}
$window = Get-Process | Where-Object {
    $_.MainWindowTitle -like "*$TitleMatch*" -and
    (-not $CommandLineMatch -or $allowed.Contains([int]$_.Id))
} | Select-Object -First 1
if ($window) { Write-Output $window.MainWindowTitle }
