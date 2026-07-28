# axio installer for Windows — https://axio.sh/install.ps1
#
#   irm https://axio.sh/install.ps1 | iex
#
# This builds axio from source with cargo. There is no published binary: no
# release has been tagged, so there is nothing to download and verify. When one
# is cut this script will prefer it and fall back to a source build.
#
# It installs into your cargo bin directory as your own user. It does not
# elevate, does not write outside CARGO_HOME, and does not modify your profile.
# Read it before running it — that advice applies to every script anyone asks
# you to pipe into a shell, including this one.

$ErrorActionPreference = 'Stop'

$Repo = 'https://github.com/umbra-me/axio'
$MinRust = [version]'1.88'
$Ref = $env:AXIO_INSTALL_REF

function Step($m) { Write-Host "==> " -ForegroundColor DarkYellow -NoNewline; Write-Host $m }
function Fail($m) { Write-Host "error: " -ForegroundColor Red -NoNewline; Write-Host $m; exit 1 }

# --- preflight ---------------------------------------------------------------

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  Fail @"
cargo not found.
  axio is built from source, so a Rust toolchain is required.
  Install one with rustup:  https://rustup.rs
  Then run this script again.
"@
}

# `cargo --version` prints e.g. "cargo 1.89.0 (abc123 2026-01-01)".
$raw = (cargo --version) -split '\s+' | Select-Object -Index 1
if (-not $raw) { Fail "could not read the version from 'cargo --version'." }

# Trim any pre-release suffix (1.90.0-nightly) so [version] can parse it.
$parsed = [version](($raw -split '-')[0])
if ($parsed -lt $MinRust) {
  Fail "Rust $raw is too old; axio needs $MinRust or newer.`n  Update with:  rustup update stable"
}

$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME '.cargo' }
$cargoBin = Join-Path $cargoHome 'bin'

Write-Host ""
Write-Host "axio " -NoNewline -ForegroundColor Yellow
Write-Host "- building from source"
Write-Host "  toolchain   Rust $raw"
Write-Host "  source      $Repo$(if ($Ref) { " @ $Ref" })"
Write-Host "  destination $cargoBin"
Write-Host ""
Write-Host "No release has been tagged yet, so there is no binary to download." -ForegroundColor DarkGray
Write-Host "This compiles the current source, which takes a few minutes." -ForegroundColor DarkGray
Write-Host ""

# --- install -----------------------------------------------------------------

Step "cargo install"
if ($Ref) {
  cargo install --git $Repo --rev $Ref --locked axio
} else {
  cargo install --git $Repo --locked axio
}
if ($LASTEXITCODE -ne 0) { Fail "cargo install failed with exit code $LASTEXITCODE." }

# --- verify ------------------------------------------------------------------

# Check the binary that was just installed, not whatever `axio` resolves to —
# reporting success from a different copy already on PATH would be a lie.
$installed = Join-Path $cargoBin 'axio.exe'
if (-not (Test-Path $installed)) { Fail "cargo reported success but $installed is missing." }

Write-Host ""
Step "installed"
& $installed --version

$onPath = Get-Command axio -ErrorAction SilentlyContinue
Write-Host ""
if ($onPath -and $onPath.Source -eq $installed) {
  Write-Host "  axio --doctor   " -NoNewline; Write-Host "what axio can see, offline" -ForegroundColor DarkGray
  Write-Host "  axio            " -NoNewline; Write-Host "start an interactive session" -ForegroundColor DarkGray
} else {
  Write-Host "  $cargoBin is not first on your PATH." -ForegroundColor Yellow
  Write-Host "  Add it, or run the binary directly:"
  Write-Host "    $installed --doctor"
}
Write-Host ""
