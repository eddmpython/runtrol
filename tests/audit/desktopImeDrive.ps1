# Drive the Windows Korean IME through the production runtrol window.
#
# The caller prepares an isolated home with one live session. Characters are sent as virtual keys rather than
# Unicode injection, so composition still passes through the configured operating system input method. The
# product trace reports composition without printing the operator's text, while the clipboard independently
# proves the characters, selection, and copy result.

[CmdletBinding(DefaultParameterSetName = "Journey")]
param(
    [Parameter(Mandatory = $true, ParameterSetName = "Journey")][string]$Exe,
    [Parameter(Mandatory = $true, ParameterSetName = "Journey")][string]$OutFile,
    [Parameter(Mandatory = $true, ParameterSetName = "Journey")][string]$ErrorFile,
    [Parameter(Mandatory = $true, ParameterSetName = "Selftest")][switch]$Selftest,
    [int]$Seconds = 20
)

Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class DesktopImeKeys {
    public const int RELEASE_ATTEMPTS = 3;
    public delegate bool EnumWindowsProc(IntPtr window, IntPtr parameter);
    public sealed class InputResult {
        public bool Complete;
        public bool TargetReleased;
        public bool ControlReleased;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)]
    public struct INPUT { public uint type; public KEYBDINPUT ki; public int pad1; public int pad2; }
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    [StructLayout(LayoutKind.Sequential)]
    public struct GUITHREADINFO {
        public int cbSize;
        public uint flags;
        public IntPtr hwndActive;
        public IntPtr hwndFocus;
        public IntPtr hwndCapture;
        public IntPtr hwndMenuOwner;
        public IntPtr hwndMoveSize;
        public IntPtr hwndCaret;
        public RECT rcCaret;
    }

    [DllImport("user32.dll", SetLastError = true)]
    public static extern uint SendInput(uint count, INPUT[] inputs, int size);
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")]
    public static extern bool BringWindowToTop(IntPtr window);
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr window, int command);
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr window, IntPtr process);
    [DllImport("user32.dll")]
    public static extern bool GetGUIThreadInfo(uint thread, ref GUITHREADINFO info);
    [DllImport("user32.dll")]
    public static extern bool EnumChildWindows(IntPtr parent, EnumWindowsProc callback, IntPtr parameter);
    [DllImport("user32.dll")]
    public static extern bool IsChild(IntPtr parent, IntPtr window);
    [DllImport("user32.dll")]
    public static extern IntPtr GetKeyboardLayout(uint thread);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr LoadKeyboardLayout(string name, uint flags);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern IntPtr SendMessageTimeout(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam,
        uint flags, uint timeout, out UIntPtr result);
    [DllImport("imm32.dll")]
    public static extern IntPtr ImmGetContext(IntPtr window);
    [DllImport("imm32.dll")]
    public static extern bool ImmReleaseContext(IntPtr window, IntPtr context);
    [DllImport("imm32.dll")]
    public static extern bool ImmGetOpenStatus(IntPtr context);
    [DllImport("imm32.dll")]
    public static extern bool ImmSetOpenStatus(IntPtr context, bool open);
    [DllImport("user32.dll")]
    public static extern bool AttachThreadInput(uint attach, uint attachTo, bool doAttach);
    [DllImport("kernel32.dll")]
    public static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")]
    public static extern short VkKeyScanW(char value);
    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr window, out RECT rectangle);
    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")]
    public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);

    public static bool Focus(IntPtr window) {
        const int SW_RESTORE = 9;
        uint mine = GetCurrentThreadId();
        for (int attempt = 0; attempt < 20; attempt++) {
            if (GetForegroundWindow() == window) return true;
            uint holder = GetWindowThreadProcessId(GetForegroundWindow(), IntPtr.Zero);
            AttachThreadInput(mine, holder, true);
            try {
                ShowWindow(window, SW_RESTORE);
                BringWindowToTop(window);
                SetForegroundWindow(window);
            } finally {
                AttachThreadInput(mine, holder, false);
            }
            System.Threading.Thread.Sleep(150);
        }
        return GetForegroundWindow() == window;
    }

    public static bool ClickComposer(IntPtr window) {
        RECT rectangle;
        if (!GetWindowRect(window, out rectangle)) return false;
        int x = rectangle.Left + ((rectangle.Right - rectangle.Left) * 67 / 100);
        int y = rectangle.Top + ((rectangle.Bottom - rectangle.Top) * 86 / 100);
        if (!SetCursorPos(x, y)) return false;
        const uint DOWN = 0x0002;
        const uint UP = 0x0004;
        mouse_event(DOWN, 0, 0, 0, UIntPtr.Zero);
        mouse_event(UP, 0, 0, 0, UIntPtr.Zero);
        return true;
    }

    public static IntPtr InputWindow(IntPtr window) {
        List<IntPtr> descendants = new List<IntPtr>();
        EnumChildWindows(window, delegate(IntPtr child, IntPtr parameter) {
            descendants.Add(child);
            return true;
        }, IntPtr.Zero);
        descendants.Add(window);
        HashSet<uint> threads = new HashSet<uint>();
        IntPtr fallback = IntPtr.Zero;
        foreach (IntPtr descendant in descendants) {
            uint thread = GetWindowThreadProcessId(descendant, IntPtr.Zero);
            if (thread == 0 || !threads.Add(thread)) continue;
            GUITHREADINFO info = new GUITHREADINFO();
            info.cbSize = Marshal.SizeOf(typeof(GUITHREADINFO));
            if (!GetGUIThreadInfo(thread, ref info) || info.hwndFocus == IntPtr.Zero) continue;
            if (info.hwndFocus != window && !IsChild(window, info.hwndFocus)) continue;
            if (fallback == IntPtr.Zero) fallback = info.hwndFocus;
            IntPtr context = ImmGetContext(info.hwndFocus);
            if (context != IntPtr.Zero) {
                ImmReleaseContext(info.hwndFocus, context);
                return info.hwndFocus;
            }
        }
        return fallback != IntPtr.Zero ? fallback : window;
    }

    public static IntPtr LoadKoreanLayout() {
        const uint KLF_NOTELLSHELL = 0x00000080;
        return LoadKeyboardLayout("00000412", KLF_NOTELLSHELL);
    }

    public static IntPtr KeyboardLayout(IntPtr window) {
        return GetKeyboardLayout(GetWindowThreadProcessId(window, IntPtr.Zero));
    }

    public static bool RequestKeyboardLayout(IntPtr window, IntPtr layout) {
        const uint WM_INPUTLANGCHANGEREQUEST = 0x0050;
        const uint SMTO_BLOCK = 0x0001;
        const uint SMTO_ABORTIFHUNG = 0x0002;
        UIntPtr result;
        IntPtr sent = SendMessageTimeout(window, WM_INPUTLANGCHANGEREQUEST, UIntPtr.Zero, layout,
            SMTO_BLOCK | SMTO_ABORTIFHUNG, 2000, out result);
        if (sent == IntPtr.Zero) return false;
        for (int attempt = 0; attempt < 10; attempt++) {
            IntPtr current = KeyboardLayout(window);
            if (LayoutsMatch(current, layout)) return true;
            System.Threading.Thread.Sleep(100);
        }
        return false;
    }

    public static bool LayoutsMatch(IntPtr current, IntPtr expected) {
        return current == expected;
    }

    const uint INPUT_KEYBOARD = 1;
    const uint KEY_UP = 0x0002;

    public static bool ReleaseKeyBounded(ushort key) {
        for (int attempt = 0; attempt < RELEASE_ATTEMPTS; attempt++) {
            INPUT[] release = new INPUT[1];
            release[0].type = INPUT_KEYBOARD;
            release[0].ki.wVk = key;
            release[0].ki.dwFlags = KEY_UP;
            uint sent = SendInput(1, release, Marshal.SizeOf(typeof(INPUT)));
            if (CompleteInput(sent, 1)) return true;
            System.Threading.Thread.Sleep(25);
        }
        return false;
    }

    public static bool CompleteInput(uint sent, uint expected) {
        return sent == expected;
    }

    public static InputResult Tap(ushort key, int pauseMs) {
        INPUT[] pair = new INPUT[2];
        pair[0].type = INPUT_KEYBOARD; pair[0].ki.wVk = key;
        pair[1].type = INPUT_KEYBOARD; pair[1].ki.wVk = key; pair[1].ki.dwFlags = KEY_UP;
        uint sent = SendInput(2, pair, Marshal.SizeOf(typeof(INPUT)));
        bool complete = CompleteInput(sent, 2);
        bool targetReleased = complete || ReleaseKeyBounded(key);
        System.Threading.Thread.Sleep(pauseMs);
        return new InputResult { Complete = complete, TargetReleased = targetReleased, ControlReleased = true };
    }

    public static InputResult TapChar(char value, int pauseMs) {
        short key = VkKeyScanW(value);
        return Tap((ushort)(key & 0xFF), pauseMs);
    }

    public static InputResult CtrlTap(ushort key, int pauseMs) {
        const ushort CONTROL = 0x11;
        INPUT[] input = new INPUT[4];
        input[0].type = INPUT_KEYBOARD; input[0].ki.wVk = CONTROL;
        input[1].type = INPUT_KEYBOARD; input[1].ki.wVk = key;
        input[2].type = INPUT_KEYBOARD; input[2].ki.wVk = key; input[2].ki.dwFlags = KEY_UP;
        input[3].type = INPUT_KEYBOARD; input[3].ki.wVk = CONTROL; input[3].ki.dwFlags = KEY_UP;
        uint sent = SendInput(4, input, Marshal.SizeOf(typeof(INPUT)));
        bool complete = CompleteInput(sent, 4);
        bool targetReleased = true;
        bool controlReleased = true;
        if (!complete) {
            targetReleased = ReleaseKeyBounded(key);
            controlReleased = ReleaseKeyBounded(CONTROL);
        }
        System.Threading.Thread.Sleep(pauseMs);
        return new InputResult {
            Complete = complete,
            TargetReleased = targetReleased,
            ControlReleased = controlReleased,
        };
    }
}
'@
Add-Type -AssemblyName System.Windows.Forms

$sequence = "dkssudgktpdy"
$expected = [string]([char]0xC548 + [char]0xB155 + [char]0xD558 + [char]0xC138 + [char]0xC694)
$CLEANUP_MAX_TOGGLES = 3
$CLEANUP_MAX_PROBES = 12
$CLEANUP_KOREAN_REPROBES = 2
$script:PendingKeyUps = [System.Collections.Generic.HashSet[int]]::new()
function Get-ClipboardText {
    for ($try = 0; $try -lt 20; $try += 1) {
        try { return (Get-Clipboard -Raw -ErrorAction Stop) } catch { Start-Sleep -Milliseconds 120 }
    }
    return $null
}

function Set-ClipboardText([string]$Text) {
    for ($try = 0; $try -lt 20; $try += 1) {
        try { Set-Clipboard -Value $Text -ErrorAction Stop; return $true } catch { Start-Sleep -Milliseconds 120 }
    }
    return $false
}

function Copy-ClipboardDataObject {
    for ($try = 0; $try -lt 20; $try += 1) {
        try {
            $source = [System.Windows.Forms.Clipboard]::GetDataObject()
            if ($null -eq $source) { return @{ WasEmpty = $true; Data = $null } }
            $copy = New-Object System.Windows.Forms.DataObject
            foreach ($format in $source.GetFormats($false)) {
                $copy.SetData($format, $false, $source.GetData($format, $false))
            }
            return @{ WasEmpty = $false; Data = $copy }
        } catch {
            Start-Sleep -Milliseconds 120
        }
    }
    throw "the existing multi-format clipboard could not be backed up"
}

function Restore-ClipboardDataObject($Backup) {
    for ($try = 0; $try -lt 20; $try += 1) {
        try {
            if ($Backup.WasEmpty) {
                [System.Windows.Forms.Clipboard]::Clear()
            } else {
                [System.Windows.Forms.Clipboard]::SetDataObject($Backup.Data, $true)
            }
            return
        } catch {
            Start-Sleep -Milliseconds 120
        }
    }
    throw "the operator's multi-format clipboard could not be restored"
}

function Input-Outcome([bool]$Complete, [bool]$TargetReleased, [bool]$ControlReleased) {
    if ($Complete) { return "complete" }
    if ($TargetReleased -and $ControlReleased) { return "partial-recovered" }
    return "partial-recovery-failed"
}

function Convert-KeyCode([int]$Key) {
    return [uint16]$Key
}

function Assert-InputResult($Result, [int]$TargetKey, [bool]$UsesControl, [string]$Purpose) {
    if ($Result.Complete) { return }
    if (-not $Result.TargetReleased) { [void]$script:PendingKeyUps.Add($TargetKey) }
    if ($UsesControl -and -not $Result.ControlReleased) { [void]$script:PendingKeyUps.Add(0x11) }
    $outcome = Input-Outcome $Result.Complete $Result.TargetReleased ($Result.ControlReleased -or -not $UsesControl)
    throw "$outcome physical key injection while $Purpose"
}

function Invoke-Tap([int]$Key, [int]$PauseMs, [string]$Purpose) {
    $result = [DesktopImeKeys]::Tap((Convert-KeyCode $Key), $PauseMs)
    Assert-InputResult $result $Key $false $Purpose
}

function Invoke-CtrlTap([int]$Key, [int]$PauseMs, [string]$Purpose) {
    $result = [DesktopImeKeys]::CtrlTap((Convert-KeyCode $Key), $PauseMs)
    Assert-InputResult $result $Key $true $Purpose
}

function Recover-PendingKeyUps {
    $failed = @()
    foreach ($key in @($script:PendingKeyUps)) {
        if ([DesktopImeKeys]::ReleaseKeyBounded((Convert-KeyCode $key))) {
            [void]$script:PendingKeyUps.Remove($key)
        } else {
            $failed += $key
        }
    }
    return $failed
}

function Type-Sequence {
    foreach ($character in $sequence.ToCharArray()) {
        $key = [DesktopImeKeys]::VkKeyScanW($character) -band 0xFF
        $result = [DesktopImeKeys]::TapChar($character, 110)
        Assert-InputResult $result $key $false "typing the physical character sequence"
    }
    Start-Sleep -Milliseconds 400
}

function Can-ReadCopiedClipboard([bool]$PlaceholderSet, [bool]$CopyInjected) {
    return $PlaceholderSet -and $CopyInjected
}

function Copy-Composer {
    $placeholderSet = Set-ClipboardText "runtrol clipboard placeholder"
    if (-not $placeholderSet) { throw "the clipboard placeholder could not be established" }
    Invoke-CtrlTap 0x41 250 "selecting composer text"
    $copyResult = [DesktopImeKeys]::CtrlTap(0x43, 500)
    Assert-InputResult $copyResult 0x43 $true "copying composer text"
    $copyInjected = $copyResult.Complete
    if (-not (Can-ReadCopiedClipboard $placeholderSet $copyInjected)) {
        throw "the copy chord was partial; target and Ctrl key-up recovery were sent"
    }
    $text = Get-ClipboardText
    if ($null -eq $text) { return "" }
    return $text
}

function Classify-ObservedText([string]$Text) {
    if ($Text -ceq $expected) { return "expected" }
    if ($Text -ceq $sequence) { return "sequence" }
    if ($Text.Contains("runtrol clipboard placeholder")) { return "placeholder" }
    if ([string]::IsNullOrEmpty($Text)) { return "empty" }
    return "other"
}

function Copy-ComposerWithRetry([IntPtr]$Window) {
    $text = ""
    for ($attempt = 0; $attempt -lt 4; $attempt += 1) {
        if ($attempt -gt 0) {
            if (-not [DesktopImeKeys]::Focus($Window)) { return $text }
            Start-Sleep -Milliseconds 200
        }
        $text = Copy-Composer
        $category = Classify-ObservedText $text
        if ($category -ne "placeholder" -and $category -ne "empty") { return $text }
        Start-Sleep -Milliseconds 200
    }
    return $text
}

function Get-ObservedShape([string]$Text) {
    $hangul = @($Text.ToCharArray() | Where-Object { [int]$_ -ge 0xAC00 -and [int]$_ -le 0xD7A3 }).Count
    $jamo = @($Text.ToCharArray() | Where-Object { [int]$_ -ge 0x3130 -and [int]$_ -le 0x318F }).Count
    $latin = @($Text.ToCharArray() | Where-Object {
        ([int]$_ -ge 0x41 -and [int]$_ -le 0x5A) -or ([int]$_ -ge 0x61 -and [int]$_ -le 0x7A)
    }).Count
    return "length=$($Text.Length),hangul=$hangul,jamo=$jamo,latin=$latin"
}

function Get-TraceMarkerCount([object[]]$Trace, [string]$Marker) {
    return @($Trace | Where-Object { $_ -eq $Marker }).Count
}

function Needs-PlainModeCleanup([bool]$OriginalLatin, [bool]$ToggleInjected) {
    return $OriginalLatin -and $ToggleInjected
}

function Cleanup-ProbeKind(
    [string]$Text,
    [string]$Category,
    [int]$CopyDelta,
    [int]$CompositionStartDelta
) {
    if ($Category -eq "sequence" -and $CopyDelta -ge 1) { return "latin-exact" }
    $allowedKorean = @($Text.ToCharArray() | Where-Object {
        ([int]$_ -ge 0x1100 -and [int]$_ -le 0x11FF) -or
        ([int]$_ -ge 0x3130 -and [int]$_ -le 0x318F) -or
        ([int]$_ -ge 0xA960 -and [int]$_ -le 0xA97F) -or
        ([int]$_ -ge 0xAC00 -and [int]$_ -le 0xD7A3) -or
        ([int]$_ -ge 0xD7B0 -and [int]$_ -le 0xD7FF)
    }).Count
    if ($Text.Length -ge 1 -and $Text.Length -le $sequence.Length -and
        $allowedKorean -eq $Text.Length -and $CopyDelta -ge 1 -and $CompositionStartDelta -ge 1) {
        return "korean-trusted"
    }
    if (Is-IncompleteLatinProbe $Text $Category) { return "latin-partial" }
    return "other"
}

function Latin-RestoreStep([string]$Kind, [int]$ToggleCount, [int]$KoreanPersistence) {
    if ($Kind -eq "latin-exact") { return @{ Action = "complete"; KoreanPersistence = 0 } }
    if ($Kind -eq "other") { return @{ Action = "fail"; KoreanPersistence = 0 } }
    if ($Kind -eq "latin-partial") { return @{ Action = "probe"; KoreanPersistence = 0 } }
    if ($Kind -eq "korean-trusted") {
        if ($ToggleCount -eq 0) { return @{ Action = "toggle"; KoreanPersistence = 0 } }
        $nextPersistence = $KoreanPersistence + 1
        if ($nextPersistence -ge $CLEANUP_KOREAN_REPROBES) {
            return @{ Action = "toggle"; KoreanPersistence = 0 }
        }
        return @{ Action = "probe"; KoreanPersistence = $nextPersistence }
    }
    return @{ Action = "fail"; KoreanPersistence = 0 }
}

function Can-InjectCleanupToggle([int]$ToggleCount) {
    return $ToggleCount -lt $CLEANUP_MAX_TOGGLES
}

function Invoke-EmptyModeProbe([IntPtr]$Window, [string]$Purpose) {
    if (-not [DesktopImeKeys]::Focus($Window)) { throw "the GUI could not regain focus for $Purpose" }
    Invoke-CtrlTap 0x41 100 "selecting text for $Purpose"
    Invoke-Tap 0x08 150 "emptying the composer for $Purpose"
    $beforeTrace = @(Get-Content $OutFile -Encoding utf8 -ErrorAction SilentlyContinue)
    $startsBefore = Get-TraceMarkerCount $beforeTrace "composer composition started"
    $copiesBefore = Get-TraceMarkerCount $beforeTrace "composer copied selection"
    Type-Sequence
    $text = Copy-ComposerWithRetry $Window
    $category = Classify-ObservedText $text
    $afterTrace = @(Get-Content $OutFile -Encoding utf8 -ErrorAction SilentlyContinue)
    $startDelta = (Get-TraceMarkerCount $afterTrace "composer composition started") - $startsBefore
    $copyDelta = (Get-TraceMarkerCount $afterTrace "composer copied selection") - $copiesBefore
    return @{
        Category = $category
        Kind = (Cleanup-ProbeKind $text $category $copyDelta $startDelta)
        CopyDelta = $copyDelta
        CompositionStartDelta = $startDelta
    }
}

function Restore-ExactLatinModeBounded([IntPtr]$Window) {
    $toggleCount = 0
    $koreanPersistence = 0
    for ($probeCount = 0; $probeCount -lt $CLEANUP_MAX_PROBES; $probeCount += 1) {
        $probe = Invoke-EmptyModeProbe $Window "exact Latin IME restoration"
        $step = Latin-RestoreStep $probe.Kind $toggleCount $koreanPersistence
        if ($step.Action -eq "complete") { return }
        if ($step.Action -eq "fail") {
            throw ("the IME cleanup probe produced untrusted $($probe.Kind) evidence; " +
                "copy_delta=$($probe.CopyDelta) composition_start_delta=$($probe.CompositionStartDelta)")
        }
        if ($step.Action -eq "toggle") {
            if (-not (Can-InjectCleanupToggle $toggleCount)) {
                throw "the IME cleanup exceeded its bounded toggle count of $CLEANUP_MAX_TOGGLES"
            }
            Invoke-CtrlTap 0x41 100 "selecting text before a cleanup mode toggle"
            Invoke-Tap 0x08 150 "emptying the composer before a cleanup mode toggle"
            Invoke-Tap 0x15 300 "restoring exact Latin IME mode"
            $toggleCount += 1
            $koreanPersistence = 0
        } else {
            $koreanPersistence = $step.KoreanPersistence
        }
    }
    throw "the IME cleanup exceeded its bounded exact probe count of $CLEANUP_MAX_PROBES"
}

function Is-IncompleteLatinProbe([string]$Text, [string]$Category) {
    if ($Category -ne "other" -or [string]::IsNullOrEmpty($Text)) { return $false }
    $latin = @($Text.ToCharArray() | Where-Object {
        ([int]$_ -ge 0x41 -and [int]$_ -le 0x5A) -or ([int]$_ -ge 0x61 -and [int]$_ -le 0x7A)
    }).Count
    return $latin -eq $Text.Length
}

if ($Selftest) {
    if ((Convert-KeyCode 0x15) -ne 0x15 -or (Convert-KeyCode 0x43) -ne 0x43) {
        throw "selftest defect: the Windows PowerShell key-code conversion wrapper failed"
    }
    if (-not (Needs-PlainModeCleanup $true $true) -or
        (Needs-PlainModeCleanup $true $false) -or (Needs-PlainModeCleanup $false $true)) {
        throw "selftest defect: injected plain toggle cleanup tracking is incomplete"
    }
    $trustedMixed = [string]([char]0x3147 + [char]0x314F + [char]0x3134)
    if ((Cleanup-ProbeKind $sequence "sequence" 1 0) -ne "latin-exact" -or
        (Cleanup-ProbeKind $expected "expected" 1 1) -ne "korean-trusted" -or
        (Cleanup-ProbeKind "dkssud" "other" 1 0) -ne "latin-partial" -or
        (Cleanup-ProbeKind $trustedMixed "other" 1 1) -ne "korean-trusted") {
        throw "selftest defect: cleanup probe evidence was misclassified"
    }
    $longKorean = $expected + $expected + $expected
    if ((Cleanup-ProbeKind $longKorean "other" 1 1) -ne "other" -or
        (Cleanup-ProbeKind "$expected UI" "other" 1 1) -ne "other" -or
        (Cleanup-ProbeKind $trustedMixed "other" 0 1) -ne "other" -or
        (Cleanup-ProbeKind $trustedMixed "other" 1 0) -ne "other") {
        throw "selftest defect: page text or marker-free Korean evidence was trusted"
    }
    $firstKorean = Latin-RestoreStep "korean-trusted" 0 0
    $ignoredOnce = Latin-RestoreStep "korean-trusted" 1 0
    $ignoredTwice = Latin-RestoreStep "korean-trusted" 1 $ignoredOnce.KoreanPersistence
    if ((Latin-RestoreStep "latin-exact" 1 0).Action -ne "complete" -or
        (Latin-RestoreStep "other" 1 0).Action -ne "fail" -or
        (Latin-RestoreStep "latin-partial" 1 0).Action -ne "probe" -or
        $firstKorean.Action -ne "toggle" -or $ignoredOnce.Action -ne "probe" -or
        $ignoredTwice.Action -ne "toggle") {
        throw "selftest defect: exact Latin, untrusted, or ignored-toggle transitions were unsafe"
    }
    if (-not (Can-InjectCleanupToggle 2) -or (Can-InjectCleanupToggle 3) -or
        $CLEANUP_MAX_PROBES -ne 12 -or $CLEANUP_KOREAN_REPROBES -ne 2) {
        throw "selftest defect: cleanup toggle or probe bounds were not enforced"
    }
    if (-not [DesktopImeKeys]::LayoutsMatch([IntPtr]::new(0x12345678), [IntPtr]::new(0x12345678)) -or
        [DesktopImeKeys]::LayoutsMatch([IntPtr]::new(0x12340412), [IntPtr]::new(0x56780412))) {
        throw "selftest defect: keyboard layout restoration did not require full handle equality"
    }
    if (-not (Can-ReadCopiedClipboard $true $true) -or
        (Can-ReadCopiedClipboard $false $true) -or (Can-ReadCopiedClipboard $true $false)) {
        throw "selftest defect: stale clipboard reads were not fail-closed"
    }
    if (-not [DesktopImeKeys]::CompleteInput(2, 2) -or [DesktopImeKeys]::CompleteInput(1, 2) -or
        -not [DesktopImeKeys]::CompleteInput(4, 4) -or [DesktopImeKeys]::CompleteInput(3, 4)) {
        throw "selftest defect: partial SendInput counts were accepted"
    }
    if ((Input-Outcome $true $true $true) -ne "complete" -or
        (Input-Outcome $false $true $true) -ne "partial-recovered" -or
        (Input-Outcome $false $false $true) -ne "partial-recovery-failed" -or
        (Input-Outcome $false $true $false) -ne "partial-recovery-failed") {
        throw "selftest defect: input recovery outcomes were not distinct"
    }
    if ([DesktopImeKeys]::RELEASE_ATTEMPTS -ne 3) {
        throw "selftest defect: key-up recovery was not bounded"
    }
    if (-not (Is-IncompleteLatinProbe "dkssudgktp" "other")) {
        throw "selftest defect: an incomplete physical Latin probe was not retryable"
    }
    if (Is-IncompleteLatinProbe $expected "expected") {
        throw "selftest defect: confirmed Korean output was treated as incomplete Latin input"
    }
    if ((Classify-ObservedText $expected) -ne "expected" -or
        (Classify-ObservedText $sequence) -ne "sequence") {
        throw "selftest defect: exact Korean or physical sequence output was rejected"
    }
    foreach ($extra in @("x$expected", "$expected!", "x$sequence", "$sequence!")) {
        if ((Classify-ObservedText $extra) -ne "other") {
            throw "selftest defect: output with extra characters was accepted as exact mode evidence"
        }
    }
    foreach ($notIncomplete in @("", "!", "dks!", $expected)) {
        if (Is-IncompleteLatinProbe $notIncomplete (Classify-ObservedText $notIncomplete)) {
            throw "selftest defect: mixed, empty, punctuation, or Korean output was retryable as incomplete Latin"
        }
    }
    Write-Host "[desktopImeDrive --selftest] OK. mode, layout, clipboard, and input safety hold."
    exit 0
}

$clipboardBefore = Copy-ClipboardDataObject
$process = $null
$window = [IntPtr]::Zero
$inputWindow = [IntPtr]::Zero
$originalLayout = [IntPtr]::Zero
$koreanLayout = [IntPtr]::Zero
$layoutRestoreRequired = $false
$imeStateCaptured = $false
$originalImeOpen = $false
$originalPlainModeWasLatin = $false
$plainToggleAttempted = $false
$plainToggleInjected = $false
$plainToggleAttempts = 0
$incompleteLatinRetries = 0
$imeStrategy = "unread"
$layoutCategory = "unread"
$modeCategory = "unread"
$copiedCategory = "unread"
$modeShape = "unread"
$copiedShape = "unread"
$trace = @()

try {
    $process = Start-Process -FilePath $Exe -ArgumentList @("gui") -RedirectStandardOutput $OutFile `
        -RedirectStandardError $ErrorFile -NoNewWindow -PassThru
    for ($wait = 0; $wait -lt 60; $wait += 1) {
        $process.Refresh()
        if ($process.MainWindowHandle -ne [IntPtr]::Zero) {
            $window = $process.MainWindowHandle
            break
        }
        if ($process.HasExited) { break }
        Start-Sleep -Milliseconds 250
    }
    if ($window -eq [IntPtr]::Zero) { throw "the production GUI never opened a window" }
    if (-not [DesktopImeKeys]::Focus($window)) { throw "the production GUI could not take foreground focus" }
    $pageReady = $false
    for ($wait = 0; $wait -lt 80; $wait += 1) {
        $trace = @(Get-Content $OutFile -Encoding utf8 -ErrorAction SilentlyContinue)
        if (@($trace | Where-Object { $_ -like "first list at *" }).Count -ge 1) {
            $pageReady = $true
            break
        }
        if ($process.HasExited) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $pageReady) { throw "the embedded production page did not render its first session list" }
    Start-Sleep -Milliseconds 500
    # Establish DOM focus once. Later coordinate clicks can hit a moved send control, while copy and IME mode
    # changes preserve the contenteditable's DOM focus across foreground restoration.
    if (-not [DesktopImeKeys]::ClickComposer($window)) { throw "the composer click could not be delivered" }
    Start-Sleep -Milliseconds 500

    # Input language belongs to the focused window thread. Request the Korean layout on that thread before
    # probing Hangul mode, then restore the exact original layout during cleanup.
    $inputWindow = [DesktopImeKeys]::InputWindow($window)
    $originalLayout = [DesktopImeKeys]::KeyboardLayout($inputWindow)
    $koreanLayout = [DesktopImeKeys]::LoadKoreanLayout()
    if ($koreanLayout -eq [IntPtr]::Zero) { throw "the Microsoft Korean keyboard layout is unavailable" }
    $layoutRestoreRequired = $true
    if (-not [DesktopImeKeys]::RequestKeyboardLayout($inputWindow, $koreanLayout)) {
        throw "the focused GUI input thread did not activate the Korean keyboard layout"
    }
    $layoutCategory = "korean"
    $inputContext = [DesktopImeKeys]::ImmGetContext($inputWindow)
    if ($inputContext -ne [IntPtr]::Zero) {
        $imeStrategy = "imm"
        try {
            $originalImeOpen = [DesktopImeKeys]::ImmGetOpenStatus($inputContext)
            $imeStateCaptured = $true
            if (-not $originalImeOpen) {
                if (-not [DesktopImeKeys]::ImmSetOpenStatus($inputContext, $true)) {
                    throw "the focused WebView input context rejected Korean IME open state"
                }
                if (-not [DesktopImeKeys]::ImmGetOpenStatus($inputContext)) {
                    throw "the focused WebView input context did not retain Korean IME open state"
                }
            }
        } finally {
            if (-not [DesktopImeKeys]::ImmReleaseContext($inputWindow, $inputContext)) {
                throw "the focused WebView input context could not be released"
            }
        }
    } else {
        $imeStrategy = "tsf-toggle-probe"
    }

    # WebView2 commonly exposes TSF without a legacy HIMC. In that case, discover the mode from physical input,
    # toggle only after a confirmed Latin probe, and require both Korean output and a new composition start.
    $modeVerified = $false
    $initialModeCategory = "unread"
    for ($probe = 0; $probe -le 3; $probe += 1) {
        if (-not [DesktopImeKeys]::Focus($window)) { throw "the production GUI lost focus during IME mode probing" }
        Invoke-CtrlTap 0x41 100 "selecting text before an IME mode probe"
        Invoke-Tap 0x08 150 "clearing text before an IME mode probe"
        $trace = @(Get-Content $OutFile -Encoding utf8 -ErrorAction SilentlyContinue)
        $startsBefore = Get-TraceMarkerCount $trace "composer composition started"
        Type-Sequence
        $modeText = Copy-ComposerWithRetry $window
        $modeCategory = Classify-ObservedText $modeText
        $modeShape = Get-ObservedShape $modeText
        $trace = @(Get-Content $OutFile -Encoding utf8 -ErrorAction SilentlyContinue)
        $newStarts = (Get-TraceMarkerCount $trace "composer composition started") - $startsBefore
        if ($initialModeCategory -eq "unread" -and $modeCategory -in @("sequence", "expected")) {
            $initialModeCategory = $modeCategory
            if ($imeStrategy -eq "tsf-toggle-probe" -and $modeCategory -eq "sequence") {
                $originalPlainModeWasLatin = $true
            }
        }
        if ($modeCategory -eq "expected" -and $newStarts -ge 1) {
            $modeVerified = $true
            break
        }
        if ($modeCategory -eq "expected") {
            throw "Korean output occurred without a new production composition start"
        }
        if (Is-IncompleteLatinProbe $modeText $modeCategory) {
            if ($incompleteLatinRetries -ge 3) {
                throw "physical-key input remained incomplete after bounded retries"
            }
            $incompleteLatinRetries += 1
            $probe -= 1
            continue
        }
        if ($modeCategory -ne "sequence") {
            throw "the IME mode probe produced an unrecognized text category"
        }
        if ($imeStrategy -eq "imm") {
            throw "the open Korean IME input context still produced the physical-key sequence"
        }
        if ($probe -eq 3) { break }
        Invoke-CtrlTap 0x41 100 "selecting text before a plain IME mode key"
        Invoke-Tap 0x08 150 "clearing text before a plain IME mode key"
        $plainToggleAttempted = $true
        Invoke-Tap 0x15 400 "sending the plain Korean IME mode key"
        $plainToggleInjected = $true
        $plainToggleAttempts += 1
    }
    if (-not $modeVerified) {
        throw "the Korean IME mode was not confirmed after three plain-key toggles"
    }

    if (-not [DesktopImeKeys]::Focus($window)) { throw "the production GUI lost foreground focus before measured composition" }
    Invoke-CtrlTap 0x41 150 "selecting text before measured composition"
    Invoke-Tap 0x08 150 "clearing text before measured composition"
    Type-Sequence
    # This Enter commits the last Hangul preedit. It must not reach ChatComposer's submit handler.
    Invoke-Tap 0x0D 500 "committing measured composition"
    if (-not [DesktopImeKeys]::Focus($window)) { throw "the production GUI lost foreground focus after composition" }
    Start-Sleep -Milliseconds 250
    $copied = Copy-ComposerWithRetry $window
    $copiedCategory = Classify-ObservedText $copied
    $copiedShape = Get-ObservedShape $copied
    Start-Sleep -Milliseconds 500

    $trace = @(Get-Content $OutFile -Encoding utf8 -ErrorAction SilentlyContinue)
    if (@($trace | Where-Object { $_ -eq "composer composition started" }).Count -lt 1) {
        throw "the production composer reported no composition start"
    }
    if (@($trace | Where-Object { $_ -eq "composer composition updated" }).Count -lt 5) {
        throw "the production composer reported too few composition updates"
    }
    if (@($trace | Where-Object { $_ -eq "composer composition ended" }).Count -lt 1) {
        throw "the production composer reported no composition end"
    }
    if (@($trace | Where-Object {
        $_ -eq "composer composing enter blocked" -or $_ -eq "composer composition commit enter blocked"
    }).Count -lt 1) {
        throw "the Enter that committed composition was not blocked from submission"
    }
    if (@($trace | Where-Object { $_ -eq "composer composition commit break blocked" }).Count -lt 1) {
        throw "the native composition commit break was not blocked"
    }
    if (@($trace | Where-Object { $_ -eq "composer copied selection" }).Count -lt 1) {
        throw "the production composer reported no copy event"
    }
    if (@($trace | Where-Object { $_ -eq "composer submitted" }).Count -ne 0) {
        throw "committing Korean composition submitted a prompt"
    }
    if ($copiedCategory -ne "expected") {
        throw "selection and copy did not return the composed Korean text"
    }

    Write-Host "[desktopImeDrive] OK. Windows IME composition, composing Enter, selection, and copy hold."
    exit 0
} catch {
    if (Test-Path -LiteralPath $OutFile) {
        $trace = @(Get-Content $OutFile -Encoding utf8 -ErrorAction SilentlyContinue)
    }
    $counts = @(
        "start=$(Get-TraceMarkerCount $trace 'composer composition started')"
        "update=$(Get-TraceMarkerCount $trace 'composer composition updated')"
        "end=$(Get-TraceMarkerCount $trace 'composer composition ended')"
        "blocked=$(Get-TraceMarkerCount $trace 'composer composing enter blocked')"
        "commit_blocked=$(Get-TraceMarkerCount $trace 'composer composition commit enter blocked')"
        "break_blocked=$(Get-TraceMarkerCount $trace 'composer composition commit break blocked')"
        "copy=$(Get-TraceMarkerCount $trace 'composer copied selection')"
        "submitted=$(Get-TraceMarkerCount $trace 'composer submitted')"
    ) -join " "
    [Console]::Error.WriteLine("[desktopImeDrive] FAIL. $($_.Exception.Message)")
    [Console]::Error.WriteLine("[desktopImeDrive] diagnostic hkl=$layoutCategory ime=$imeStrategy toggles=$plainToggleAttempts toggle_attempted=$plainToggleAttempted toggle_injected=$plainToggleInjected latin_retries=$incompleteLatinRetries mode=$modeCategory($modeShape) copied=$copiedCategory($copiedShape) markers: $counts")
    exit 2
} finally {
    $cleanupErrors = @()
    [void](Recover-PendingKeyUps)
    $keyboardStateReady = $script:PendingKeyUps.Count -eq 0
    if ((Needs-PlainModeCleanup $originalPlainModeWasLatin ($plainToggleAttempted -or $plainToggleInjected)) -and
        $keyboardStateReady) {
        if ($process -and -not $process.HasExited -and $window -ne [IntPtr]::Zero -and [DesktopImeKeys]::Focus($window)) {
            try {
                Restore-ExactLatinModeBounded $window
            } catch {
                $cleanupErrors += $_.Exception.Message
            }
        } else {
            $cleanupErrors += "the GUI could not regain composer focus for IME mode restoration"
        }
    } elseif (Needs-PlainModeCleanup $originalPlainModeWasLatin ($plainToggleAttempted -or $plainToggleInjected)) {
        $cleanupErrors += "the IME mode could not be restored while operator key-up recovery was incomplete"
    }
    $failedKeyUps = @(Recover-PendingKeyUps)
    if ($failedKeyUps.Count -gt 0) {
        $cleanupErrors += "operator keyboard state cleanup failed for $($failedKeyUps.Count) key-up events"
    }
    if ($imeStateCaptured) {
        if ($process -and -not $process.HasExited -and $inputWindow -ne [IntPtr]::Zero) {
            $restoreContext = [DesktopImeKeys]::ImmGetContext($inputWindow)
            if ($restoreContext -eq [IntPtr]::Zero) {
                $cleanupErrors += "the WebView IME input context was unavailable for state restoration"
            } else {
                try {
                    if ([DesktopImeKeys]::ImmGetOpenStatus($restoreContext) -ne $originalImeOpen) {
                        if (-not [DesktopImeKeys]::ImmSetOpenStatus($restoreContext, $originalImeOpen)) {
                            $cleanupErrors += "the WebView IME open state could not be restored"
                        }
                    }
                    if ([DesktopImeKeys]::ImmGetOpenStatus($restoreContext) -ne $originalImeOpen) {
                        $cleanupErrors += "the WebView IME open state restoration could not be verified"
                    }
                } finally {
                    if (-not [DesktopImeKeys]::ImmReleaseContext($inputWindow, $restoreContext)) {
                        $cleanupErrors += "the restored WebView IME input context could not be released"
                    }
                }
            }
        } else {
            $cleanupErrors += "the GUI exited before its original IME open state could be restored"
        }
    }
    if ($layoutRestoreRequired) {
        if ($process -and -not $process.HasExited -and $inputWindow -ne [IntPtr]::Zero) {
            if (-not [DesktopImeKeys]::RequestKeyboardLayout($inputWindow, $originalLayout)) {
                $cleanupErrors += "the GUI input thread's original keyboard layout could not be restored"
            }
        } else {
            $cleanupErrors += "the GUI exited before its original keyboard layout could be restored"
        }
    }
    try {
        Restore-ClipboardDataObject $clipboardBefore
    } catch {
        $cleanupErrors += $_.Exception.Message
    }
    if ($process -and -not $process.HasExited) {
        $process.CloseMainWindow() | Out-Null
        if (-not $process.WaitForExit(3000)) {
            $process.Kill()
            $process.WaitForExit()
        }
    }
    if ($cleanupErrors.Count -gt 0) {
        [Console]::Error.WriteLine("[desktopImeDrive] CLEANUP FAIL. " + ($cleanupErrors -join "; "))
        exit 2
    }
}
