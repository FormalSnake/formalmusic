# Builds kopuzd at the Kopuz tag Cargo.toml pins into target/kopuzd, then packs
# the app with it beside formalmusic.exe (crates/app/nativedesktop.toml lists it).
#
#   scripts/package-windows.ps1 [nd package args]
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tag = (Select-String -Path "$root\Cargo.toml" -Pattern 'Kopuz-org/kopuz", tag = "([^"]+)"' |
    Select-Object -First 1).Matches.Groups[1].Value
if (-not $tag) { throw 'no Kopuz tag in Cargo.toml' }
cargo install --locked --git https://github.com/Kopuz-org/kopuz --tag $tag kopuz-kopuzd --root "$root\target\kopuzd"
if ($LASTEXITCODE) { throw "building kopuzd $tag failed" }
nd package windows --dir "$root\crates\app" @args
if ($LASTEXITCODE) { throw 'nd package windows failed' }
