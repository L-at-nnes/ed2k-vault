<#
.SYNOPSIS
  Builds a portable ed2k-vault folder (and zip) in ./portable.
.DESCRIPTION
  Runs a release build, then copies the CLI, the GUI, the licence and the
  readmes into portable/ed2k-vault-<version>/ and zips it.
  A "portable.txt" file is not needed: the app keeps its cache in the user cache
  folder unless you pass --cache (CLI) or untick the cache (GUI).
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build --release --workspace
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$version = (Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
$name = "ed2k-vault-$version-windows-x64"
$out = Join-Path $root "portable\$name"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null

foreach ($file in 'ed2k-vault.exe', 'ed2k-vault-gui.exe') {
    Copy-Item (Join-Path $root "target\release\$file") $out
}
Copy-Item LICENSE, README.md, README_FR.md $out

$zip = Join-Path $root "portable\$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $out -DestinationPath $zip

Write-Host "Portable build ready:`n  $out`n  $zip"
