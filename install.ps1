[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$repository = $env:WUSTITE_REPOSITORY
if ([string]::IsNullOrWhiteSpace($repository)) {
    $repository = "https://github.com/Hunter2030ZeRo/Wustite"
}

$installRoot = $env:WUSTITE_INSTALL_ROOT
if ([string]::IsNullOrWhiteSpace($installRoot)) {
    $installRoot = $env:CARGO_INSTALL_ROOT
}

if ($null -eq (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Cargo is required. Install Rust from https://rustup.rs/ and try again."
}

$cargoArguments = @("install", "--git", $repository, "--locked", "--force")
if (-not [string]::IsNullOrWhiteSpace($installRoot)) {
    $cargoArguments += @("--root", $installRoot)
}
$cargoArguments += "wustite"

Write-Host "Installing Wustite from $repository"
& cargo @cargoArguments
if ($LASTEXITCODE -ne 0) {
    throw "Cargo failed to install Wustite (exit code $LASTEXITCODE)."
}

if (-not [string]::IsNullOrWhiteSpace($installRoot)) {
    $binDirectory = Join-Path $installRoot "bin"
} elseif (-not [string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
    $binDirectory = Join-Path $env:CARGO_HOME "bin"
} else {
    $binDirectory = Join-Path $env:USERPROFILE ".cargo\bin"
}

$executable = Join-Path $binDirectory "wustite.exe"
Write-Host "Wustite installed at $executable"
if (($env:PATH -split [IO.Path]::PathSeparator) -notcontains $binDirectory) {
    Write-Host "Add $binDirectory to PATH to run wustite from any directory."
}
