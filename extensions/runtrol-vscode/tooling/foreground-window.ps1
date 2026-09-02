# Prints the title and process id of the window Windows counts as the foreground window right now, as one line
# "<pid> <title>", or nothing when no window has the foreground. Read-only: the eye passes use it to judge whether
# an owner reveal (`EXT-03`) actually brought a window forward.
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class RuntrolForegroundWin32 {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr handle, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr handle, out uint processId);
}
"@
$handle = [RuntrolForegroundWin32]::GetForegroundWindow()
if ($handle -eq [IntPtr]::Zero) { return }
$text = New-Object System.Text.StringBuilder 1024
[RuntrolForegroundWin32]::GetWindowText($handle, $text, 1024) | Out-Null
$processId = [uint32]0
[RuntrolForegroundWin32]::GetWindowThreadProcessId($handle, [ref]$processId) | Out-Null
Write-Output ("{0} {1}" -f $processId, $text.ToString())
