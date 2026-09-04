# Prints every visible titled top-level window as one "<pid>|<title>" line. Read-only; the eye passes log which
# windows a launched terminal host owns before they type into it (measured 2026-09-05: a console window belongs to
# its shell process, a Windows Terminal window to WindowsTerminal.exe).
. (Join-Path $PSScriptRoot "find-window.ps1")
[RuntrolWindowWin32]::Visible() | ForEach-Object { Write-Output ("{0}|{1}" -f $_.ProcessId, $_.Title) }
