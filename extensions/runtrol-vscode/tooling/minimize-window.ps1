# Minimise one window the caller owns, found by its owning process (or a title), so an eye pass can prove that a
# reveal restores and raises it rather than finding it already in front. Read-only apart from that one request,
# which Windows may refuse; the script says which happened.
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
if (-not $window) {
    Write-Error "no window has a title matching '$wantedTitle' (process $wantedProcess)"
    exit 2
}
# 6 = SW_MINIMIZE. The request is asynchronous for a window another process owns, so the state is read back.
$handle = [IntPtr]::new([long]$window.Handle)
[RuntrolWindowWin32]::ShowWindow($handle, 6) | Out-Null
$iconic = $false
for ($attempt = 0; $attempt -lt 10 -and -not $iconic; $attempt += 1) {
    Start-Sleep -Milliseconds 200
    $iconic = [RuntrolWindowWin32]::IsIconic($handle)
}
Write-Output ("minimised '{0}' iconic={1}" -f $window.Title, $iconic)
