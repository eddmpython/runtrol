$ErrorActionPreference = 'Stop'

# An opened handle pins one process generation. Never resolve the PID again between checking and terminating it.
# TerminateProcess is asynchronous; only the same handle becoming signaled proves that generation has exited.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

public sealed class OwnedProcess {
    public int pid;
    public long startedAt;
    public string executable;
}

public sealed class ProcessOutcome {
    public int pid;
    public string state;
    public int error;
}

public static class ExactProcessTermination {
    private const uint QueryLimited = 0x1000;
    private const uint Terminate = 0x0001;
    private const uint Synchronize = 0x00100000;
    private const uint Signaled = 0;
    private const uint Timeout = 258;
    private const long UnixEpochFileTime = 116444736000000000;

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetProcessTimes(IntPtr process, out long created, out long exited, out long kernel, out long user);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool QueryFullProcessImageName(IntPtr process, uint flags, StringBuilder name, ref uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);

    public static ProcessOutcome[] Run(OwnedProcess[] identities, int protectedPid, uint waitMs) {
        // Reject an incomplete batch before any side effect. Unknown creation time is never ownership proof.
        foreach (OwnedProcess identity in identities) {
            if (identity.pid <= 0 || identity.pid == protectedPid || identity.pid == Process.GetCurrentProcess().Id
                || identity.startedAt <= 0 || String.IsNullOrEmpty(identity.executable)
                || !Path.IsPathRooted(identity.executable)) {
                throw new ArgumentException("process termination requires a known PID, birth, and absolute image path");
            }
        }
        var outcomes = new List<ProcessOutcome>();
        var handles = new List<IntPtr>();
        var pending = new List<KeyValuePair<IntPtr, ProcessOutcome>>();
        var elapsed = Stopwatch.StartNew();
        try {
            foreach (OwnedProcess identity in identities) {
                var outcome = new ProcessOutcome { pid = identity.pid };
                outcomes.Add(outcome);
                IntPtr handle = OpenProcess(QueryLimited | Terminate | Synchronize, false, identity.pid);
                if (handle == IntPtr.Zero) {
                    outcome.error = Marshal.GetLastWin32Error();
                    outcome.state = outcome.error == 87 ? "absent" : "failed";
                    continue;
                }
                // Keep every successfully opened handle until all termination requests have been issued and waited.
                handles.Add(handle);
                if (WaitForSingleObject(handle, 0) == Signaled) {
                    outcome.state = "exited";
                    continue;
                }
                long created, exited, kernel, user;
                var image = new StringBuilder(32768);
                uint size = (uint)image.Capacity;
                if (!GetProcessTimes(handle, out created, out exited, out kernel, out user)
                    || !QueryFullProcessImageName(handle, 0, image, ref size)) {
                    outcome.error = Marshal.GetLastWin32Error();
                    outcome.state = WaitForSingleObject(handle, 0) == Signaled ? "exited" : "failed";
                    continue;
                }
                if ((created - UnixEpochFileTime) / 10000 != identity.startedAt
                    || !String.Equals(Path.GetFullPath(image.ToString()), Path.GetFullPath(identity.executable), StringComparison.OrdinalIgnoreCase)) {
                    outcome.state = "mismatch";
                    continue;
                }
                if (!TerminateProcess(handle, 1)) outcome.error = Marshal.GetLastWin32Error();
                outcome.state = "pending";
                pending.Add(new KeyValuePair<IntPtr, ProcessOutcome>(handle, outcome));
            }
            // One batch deadline, not a fresh timeout for every child in the tree.
            foreach (var entry in pending) {
                uint remaining = (uint)Math.Max(0, (long)waitMs - elapsed.ElapsedMilliseconds);
                uint waited = WaitForSingleObject(entry.Key, remaining);
                if (waited == Signaled) entry.Value.state = "exited";
                else if (waited == Timeout) entry.Value.state = "pending";
                else {
                    entry.Value.state = "failed";
                    entry.Value.error = Marshal.GetLastWin32Error();
                }
            }
            return outcomes.ToArray();
        } finally {
            int closeError = 0;
            foreach (IntPtr handle in handles) {
                if (!CloseHandle(handle)) closeError = Marshal.GetLastWin32Error();
            }
            if (closeError != 0) throw new Win32Exception(closeError, "cannot close owned process handle");
        }
    }
}
'@

$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$identities = @($request.identities | ForEach-Object {
    $identity = New-Object OwnedProcess
    $identity.pid = $_.pid
    $identity.startedAt = $_.startedAt
    $identity.executable = $_.executable
    $identity
})
$outcomes = [ExactProcessTermination]::Run([OwnedProcess[]]$identities, $request.protectedPid, $request.waitMs)
ConvertTo-Json -InputObject @($outcomes) -Compress
