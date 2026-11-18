$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# Starts the packaged program as a user would, from the unpacked zip, and sees that it stays
# up and opens its window. A program that cannot start on Windows fails here, not on the
# computer of whoever downloaded it.

$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$archive = Get-ChildItem (Join-Path $root "target\windows\*.zip") -File | Select-Object -First 1
if (-not $archive) {
    throw "no packaged zip in target\windows; run package.ps1 first"
}
$unpacked = Join-Path ([System.IO.Path]::GetTempPath()) "openpickle-smoke"
Remove-Item $unpacked -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive $archive.FullName $unpacked

$process = Start-Process (Join-Path $unpacked "OpenPickle.exe") -PassThru
try {
    $opened = $false
    $deadline = (Get-Date).AddSeconds(30)
    while (-not $opened -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
        $process.Refresh()
        if ($process.HasExited) {
            throw "OpenPickle.exe stopped with exit code $($process.ExitCode) right after it started"
        }
        $opened = $process.MainWindowHandle -ne [IntPtr]::Zero
    }
    if (-not $opened) {
        throw "OpenPickle.exe opened no window within 30 seconds"
    }
    Write-Output "OpenPickle.exe is running with the window '$($process.MainWindowTitle)'"
}
finally {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
}
