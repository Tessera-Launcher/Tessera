param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Path
)

# Compression for the Windows release artifact. UPX is pinned so the same
# executable is produced on every run and the download can be verified.
$version = '5.3.1'
$sha256 = '9ff51d1646153b33e8c595b02a4f042fe76be6e7f0abcb752918373ac1f9af04'

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$target = (Resolve-Path -LiteralPath $Path).Path
$root = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$dir = Join-Path $root "upx-$version"
$archive = Join-Path $dir "upx-$version-win64.zip"
New-Item -ItemType Directory -Force -Path $dir | Out-Null

if (-not (Test-Path -LiteralPath $archive)) {
    Invoke-WebRequest `
        -Uri "https://github.com/upx/upx/releases/download/v$version/upx-$version-win64.zip" `
        -OutFile $archive
}

$actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $sha256) {
    throw "UPX archive checksum mismatch: expected $sha256, got $actual"
}

Expand-Archive -LiteralPath $archive -DestinationPath $dir -Force
$upx = Get-ChildItem -LiteralPath $dir -Recurse -Filter upx.exe | Select-Object -First 1
if (-not $upx) {
    throw "upx.exe not found in $archive"
}

# The build cache can restore an executable that an earlier run already packed,
# and UPX refuses to pack a file twice. A packed binary carries the `UPX0` and
# `UPX1` section headers, so detect those and leave the file alone.
$image = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($target))
if ($image.Contains('UPX0') -and $image.Contains('UPX1')) {
    Write-Host "UPX: $Path is already packed, skipping"
    return
}

$before = (Get-Item -LiteralPath $target).Length
& $upx.FullName --best --lzma $target
if ($LASTEXITCODE -ne 0) {
    throw "UPX failed with exit code $LASTEXITCODE"
}
& $upx.FullName -t $target
if ($LASTEXITCODE -ne 0) {
    throw "UPX self-test failed with exit code $LASTEXITCODE"
}

$after = (Get-Item -LiteralPath $target).Length
Write-Host ("UPX: {0:N0} -> {1:N0} bytes ({2:P1})" -f $before, $after, ($after / $before))
