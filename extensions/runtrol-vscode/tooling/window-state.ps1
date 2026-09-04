# Prints the state of one window found by its owning process (or a title) as "<pid>|<iconic>|<title>", or nothing
# when no such window is visible. Read-only: an eye pass reads it before and after a reveal to prove the window was
# minimised and is not any more.
param(
    [string]$TitleMatch = "",
    [string]$CommandLineMatch = "",
    [int]$ProcessId = 0
)
$ErrorActionPreference = "Stop"
$wantedTitle = $TitleMatch
$wantedFamily = $CommandLineMatch
$wantedProcess = $ProcessId
. (Join-Path $PSScriptRoot "find-window.ps1")
$window = Find-RuntrolWindow $wantedTitle $wantedFamily $wantedProcess
if (-not $window) { return }
$iconic = [RuntrolWindowWin32]::IsIconic([IntPtr]::new([long]$window.Handle))
Write-Output ("{0}|{1}|{2}" -f $window.ProcessId, $iconic, $window.Title)
