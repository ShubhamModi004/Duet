# Run Duet from source on Windows.
# Needs: Rust (https://rustup.rs), Visual Studio Build Tools with "Desktop development with C++", Node.js.
param([ValidateSet("dev", "build")] [string]$Mode = "dev")
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\app\src-tauri")
npx --yes "@tauri-apps/cli@2" $Mode
