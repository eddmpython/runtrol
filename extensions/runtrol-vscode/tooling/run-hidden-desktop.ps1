param(
    [Parameter(Mandatory = $true)]
    [string] $Executable,

    [Parameter(Mandatory = $true)]
    [string] $WorkingDirectory,

    [Parameter(Mandatory = $true)]
    [string] $Argument
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class RuntrolHiddenDesktop
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct StartupInfo
    {
        public int cb;
        public string lpReserved;
        public string lpDesktop;
        public string lpTitle;
        public int dwX;
        public int dwY;
        public int dwXSize;
        public int dwYSize;
        public int dwXCountChars;
        public int dwYCountChars;
        public int dwFillAttribute;
        public int dwFlags;
        public short wShowWindow;
        public short cbReserved2;
        public IntPtr lpReserved2;
        public IntPtr hStdInput;
        public IntPtr hStdOutput;
        public IntPtr hStdError;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct ProcessInformation
    {
        public IntPtr hProcess;
        public IntPtr hThread;
        public int dwProcessId;
        public int dwThreadId;
    }

    [DllImport("user32.dll", EntryPoint = "CreateDesktopW", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    public static extern IntPtr CreateDesktop(
        string desktop,
        IntPtr device,
        IntPtr deviceMode,
        int flags,
        int desiredAccess,
        IntPtr securityAttributes
    );

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool CloseDesktop(IntPtr desktop);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool CreateProcess(
        string applicationName,
        StringBuilder commandLine,
        IntPtr processAttributes,
        IntPtr threadAttributes,
        [MarshalAs(UnmanagedType.Bool)] bool inheritHandles,
        int creationFlags,
        IntPtr environment,
        string currentDirectory,
        ref StartupInfo startupInfo,
        out ProcessInformation processInformation
    );

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);

    [DllImport("kernel32.dll")]
    public static extern IntPtr GetStdHandle(int standardHandle);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool CloseHandle(IntPtr handle);
}
'@

$desktopName = "RuntrolTests-$PID-$([Guid]::NewGuid().ToString('N'))"
$desktop = [RuntrolHiddenDesktop]::CreateDesktop($desktopName, [IntPtr]::Zero, [IntPtr]::Zero, 0, 0x0002, [IntPtr]::Zero)
if ($desktop -eq [IntPtr]::Zero) {
    throw "CreateDesktop failed with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
}

$process = New-Object RuntrolHiddenDesktop+ProcessInformation
try {
    $startup = New-Object RuntrolHiddenDesktop+StartupInfo
    $startup.cb = [Runtime.InteropServices.Marshal]::SizeOf([type][RuntrolHiddenDesktop+StartupInfo])
    $startup.lpDesktop = $desktopName
    $startup.dwFlags = 0x00000100
    $startup.hStdInput = [RuntrolHiddenDesktop]::GetStdHandle(-10)
    $startup.hStdOutput = [RuntrolHiddenDesktop]::GetStdHandle(-11)
    $startup.hStdError = [RuntrolHiddenDesktop]::GetStdHandle(-12)

    $commandLine = New-Object Text.StringBuilder
    [void] $commandLine.Append('"').Append($Executable).Append('" "').Append($Argument).Append('"')
    $env:RUNTROL_VSCODE_HIDDEN_DESKTOP = "1"
    $started = [RuntrolHiddenDesktop]::CreateProcess(
        $Executable,
        $commandLine,
        [IntPtr]::Zero,
        [IntPtr]::Zero,
        $true,
        0,
        [IntPtr]::Zero,
        $WorkingDirectory,
        [ref] $startup,
        [ref] $process
    )
    if (-not $started) {
        throw "CreateProcess failed with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    [void] [RuntrolHiddenDesktop]::WaitForSingleObject($process.hProcess, [uint32]::MaxValue)
    [uint32] $exitCode = 1
    if (-not [RuntrolHiddenDesktop]::GetExitCodeProcess($process.hProcess, [ref] $exitCode)) {
        throw "GetExitCodeProcess failed with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    exit $exitCode
}
finally {
    if ($process.hThread -ne [IntPtr]::Zero) {
        [void] [RuntrolHiddenDesktop]::CloseHandle($process.hThread)
    }
    if ($process.hProcess -ne [IntPtr]::Zero) {
        [void] [RuntrolHiddenDesktop]::CloseHandle($process.hProcess)
    }
    [void] [RuntrolHiddenDesktop]::CloseDesktop($desktop)
}
