param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"
if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) {
    throw "smoke-windows requires Windows"
}

$repository = Split-Path -Parent $PSScriptRoot
$binary = Join-Path $repository "target\$Profile\cleanerx-app.exe"
$log = Join-Path $repository "target\windows-smoke.log"
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "missing $binary"
}

$process = Start-Process `
    -FilePath $binary `
    -PassThru `
    -RedirectStandardOutput $log `
    -RedirectStandardError "$log.stderr"

try {
    Start-Sleep -Seconds 8
    $process.Refresh()
    if ($process.HasExited) {
        $stderr = if (Test-Path -LiteralPath "$log.stderr") {
            Get-Content -LiteralPath "$log.stderr" -Raw
        } else {
            ""
        }
        throw "CleanerX exited during the smoke window with code $($process.ExitCode). $stderr"
    }
} finally {
    if (-not $process.HasExited) {
        $null = $process.CloseMainWindow()
        if (-not $process.WaitForExit(5000)) {
            Stop-Process -Id $process.Id
        }
    }
}
