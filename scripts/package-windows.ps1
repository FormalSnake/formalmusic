# Builds kopuzd at the Kopuz rev Cargo.toml pins into target/kopuzd, then packs
# the app with it beside formalmusic.exe (crates/app/nativedesktop.toml lists it).
#
#   scripts/package-windows.ps1 [nd package args]
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$rev = (Select-String -Path "$root\Cargo.toml" -Pattern 'Kopuz-org/kopuz", rev = "([^"]+)"' |
    Select-Object -First 1).Matches.Groups[1].Value
if (-not $rev) { throw 'no Kopuz rev in Cargo.toml' }
cargo install --locked --git https://github.com/Kopuz-org/kopuz --rev $rev kopuz-kopuzd --root "$root\target\kopuzd"
if ($LASTEXITCODE) { throw "building kopuzd $rev failed" }
nd package windows --dir "$root\crates\app" @args
if ($LASTEXITCODE) { throw 'nd package windows failed' }
