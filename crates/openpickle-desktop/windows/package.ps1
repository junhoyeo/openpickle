$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$target = "x86_64-pc-windows-msvc"
$out = Join-Path $root "target\windows"
$stage = Join-Path $out "image"

Push-Location $root
try {
    if (-not $env:GPUI_FXC_PATH) {
        $fxcPattern = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin\*\x64\fxc.exe"
        $fxc = Get-ChildItem $fxcPattern -File | Sort-Object FullName -Descending | Select-Object -First 1
        if (-not $fxc) {
            throw "fxc.exe was not found in the Windows 10 SDK"
        }
        $env:GPUI_FXC_PATH = $fxc.FullName
    }

    $metadata = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
    $version = ($metadata.packages | Where-Object name -EQ "openpickle-desktop").version
    $archive = Join-Path $out "OpenPickle-$version-windows-x86_64.zip"

    cargo build --locked --release --target $target -p openpickle-desktop --bin openpickle
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }

    Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
    New-Item $stage -ItemType Directory -Force | Out-Null
    Copy-Item "target\$target\release\openpickle.exe" (Join-Path $stage "OpenPickle.exe")
    Copy-Item "crates\openpickle-desktop\windows\README.txt" $stage
    Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $archive -CompressionLevel Optimal
    Write-Output $archive
}
finally {
    Pop-Location
}
