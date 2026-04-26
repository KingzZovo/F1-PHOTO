<#
.SYNOPSIS
  Build a Windows release zip for F1-Photo.

.DESCRIPTION
  Run from a Windows host with Rust + Node + a portable PG 16 + pgvector tree
  staged under .\bundled-pg\, ONNX runtime under .\runtime\, and NSSM at
  .\packaging\windows\nssm.exe.

  Output: dist\f1photo-<version>-windows-x86_64.zip
#>
$ErrorActionPreference = 'Stop'
$Root = Resolve-Path "$PSScriptRoot\..\.."
Set-Location $Root

$Version = (Select-String -Path "server\Cargo.toml" -Pattern '^version').Line
$Version = ($Version -replace '.*"(.*)"', '$1')

Write-Host "[1/4] building Vue 3 web/dist"
Push-Location web
npm install --no-audit --no-fund
npx vite build
Pop-Location

Write-Host "[2/4] cargo build --release"
Push-Location server
cargo build --release --bin f1photo
Pop-Location

$Dist = Join-Path $Root "dist\f1photo-$Version-windows"
if (Test-Path $Dist) { Remove-Item $Dist -Recurse -Force }
New-Item -ItemType Directory $Dist | Out-Null
New-Item -ItemType Directory "$Dist\payload" | Out-Null
New-Item -ItemType Directory "$Dist\packaging" | Out-Null

Write-Host "[3/4] assembling payload at $Dist"
Copy-Item server\target\release\f1photo.exe "$Dist\payload\f1photo.exe"
Copy-Item server\migrations "$Dist\payload\migrations" -Recurse
foreach ($d in @('models','runtime','bundled-pg','data','logs')) {
    if (Test-Path $d) {
        Copy-Item $d "$Dist\payload\$d" -Recurse
    } else {
        New-Item -ItemType Directory "$Dist\payload\$d" | Out-Null
    }
}
Copy-Item packaging\windows "$Dist\packaging\windows" -Recurse
Copy-Item packaging\scripts "$Dist\packaging\scripts" -Recurse

Write-Host "[4/4] zipping"
$Zip = "$Root\dist\f1photo-$Version-windows-x86_64.zip"
if (Test-Path $Zip) { Remove-Item $Zip -Force }
Compress-Archive -Path "$Dist\*" -DestinationPath $Zip -CompressionLevel Optimal
Write-Host "OK -> $Zip"
