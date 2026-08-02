# Interactive Windows gate for the production Astryx composer.
#
# This is intentionally separate from hosted browser coverage. It needs a signed-in Windows desktop with the
# Microsoft Korean IME installed because virtual keyboard input, foreground focus, and the operating system
# clipboard are the product behaviours being measured.
# Pass -BuiltProduct only after the release custom-protocol product and release ACP fixture were built together.

param([switch]$BuiltProduct)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$suffix = ".exe"
$runtrol = Join-Path $root "target/release/runtrol$suffix"
$fixture = Join-Path $root "target/release/examples/acpFixture$suffix"
$gateHome = Join-Path ([System.IO.Path]::GetTempPath()) ("runtrol-ime-" + [Guid]::NewGuid().ToString("N"))
$workspace = Join-Path $gateHome "workspace"
$providers = Join-Path $gateHome "providers"
$stdout = Join-Path $gateHome "gui.stdout"
$stderr = Join-Path $gateHome "gui.stderr"
$daemonOut = Join-Path $gateHome "daemon.stdout"
$daemonErr = Join-Path $gateHome "daemon.stderr"
$oldHome = $env:RUNTROL_HOME
$oldPath = $env:PATH
$oldTrace = $env:RUNTROL_GUI_TRACE
$daemon = $null
$session = $null

try {
    if (-not $BuiltProduct) {
        $ui = Join-Path $root "crates/runtrol-gui/ui"
        & npm.cmd --prefix $ui run build
        if ($LASTEXITCODE -ne 0) { throw "the production desktop bundle did not build" }
        & cargo build --release -p runtrol --bin runtrol --features runtrol-gui/custom-protocol
        if ($LASTEXITCODE -ne 0) { throw "the production executable did not build" }
        & cargo build --release -p runtrol-drivers --example acpFixture
        if ($LASTEXITCODE -ne 0) { throw "the release ACP fixture did not build" }
    }
    if (-not (Test-Path -LiteralPath $runtrol -PathType Leaf)) {
        throw "the release product executable is missing"
    }
    if (-not (Test-Path -LiteralPath $fixture -PathType Leaf)) {
        throw "the release ACP fixture is missing"
    }

    New-Item -ItemType Directory -Path $workspace -Force | Out-Null
    New-Item -ItemType Directory -Path $providers -Force | Out-Null
    $manifest = @"
schema = 1
id = "fixture-acp"
display_name = "ACP Fixture"
kind = "acp"

[bin]
names = ["$(Split-Path $fixture -Leaf)"]

[probe]
version = { args = ["--version"], parse = "semver-anywhere" }

[transport]
argv = []
listen = "stdio"
"@
    Set-Content -Path (Join-Path $providers "fixture-acp.toml") -Value $manifest -Encoding utf8

    $env:RUNTROL_HOME = $gateHome
    $env:PATH = "$(Split-Path $fixture -Parent);$oldPath"
    $env:RUNTROL_GUI_TRACE = "1"
    $daemon = Start-Process -FilePath $runtrol -ArgumentList @("daemon") -RedirectStandardOutput $daemonOut `
        -RedirectStandardError $daemonErr -NoNewWindow -PassThru
    $ready = Join-Path $gateHome "runtrol.redb"
    for ($wait = 0; $wait -lt 80 -and -not (Test-Path $ready); $wait += 1) {
        if ($daemon.HasExited) { throw "the isolated daemon exited before it became ready" }
        Start-Sleep -Milliseconds 100
    }
    if (-not (Test-Path $ready)) { throw "the isolated daemon did not become ready" }
    Start-Sleep -Milliseconds 200

    $session = (& $runtrol start fixture-acp $workspace | Select-Object -Last 1).Trim()
    if ($LASTEXITCODE -ne 0 -or $session -notmatch "^[0-9a-f-]{36}$") {
        throw "the isolated ACP session did not start"
    }

    $shell = (Get-Process -Id $PID).Path
    & $shell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "desktopImeDrive.ps1") `
        -Exe $runtrol -OutFile $stdout -ErrorFile $stderr
    if ($LASTEXITCODE -ne 0) {
        if (Test-Path -LiteralPath $stdout) {
            [Console]::Error.WriteLine("[desktopImeSmoke] GUI stdout content-free trace:")
            $safeTrace = @(Get-Content $stdout -Encoding utf8 | Where-Object {
                $_ -like "first list at *" -or $_ -like "composer composition *" -or
                $_ -eq "composer composing enter blocked" -or $_ -eq "composer copied selection" -or
                $_ -eq "composer submitted"
            })
            if ($safeTrace.Count -eq 0) {
                [Console]::Error.WriteLine("[desktopImeSmoke] <no content-free GUI trace markers>")
            } else {
                foreach ($line in $safeTrace) { [Console]::Error.WriteLine("[desktopImeSmoke] $line") }
            }
        }
        if (Test-Path -LiteralPath $stderr) {
            [Console]::Error.WriteLine("[desktopImeSmoke] GUI stderr:")
            foreach ($line in @(Get-Content $stderr -Encoding utf8)) {
                [Console]::Error.WriteLine("[desktopImeSmoke] $line")
            }
        }
        throw "the production Windows IME journey failed"
    }
    Write-Host "[desktopImeSmoke] OK. the production window passed the interactive Windows IME gate."
    exit 0
} catch {
    [Console]::Error.WriteLine("[desktopImeSmoke] FAIL. $($_.Exception.Message)")
    exit 2
} finally {
    try {
        if ($session) { & $runtrol close $session --now | Out-Null }
    } finally {
        try {
            if ($daemon -and -not $daemon.HasExited) {
                $daemon.Kill()
                $daemon.WaitForExit()
            }
        } finally {
            $env:RUNTROL_HOME = $oldHome
            $env:PATH = $oldPath
            $env:RUNTROL_GUI_TRACE = $oldTrace
            if (Test-Path $gateHome) { Remove-Item -LiteralPath $gateHome -Recurse -Force }
        }
    }
}
