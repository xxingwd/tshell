param(
    [string]$Executable = 'target/release/tshell.exe',
    [string]$OutputDirectory = 'dist'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Set-Location (Split-Path $PSScriptRoot -Parent)
if (!(Test-Path -LiteralPath $Executable -PathType Leaf)) { throw "Executable not found: $Executable" }
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Output directory already exists; choose a new directory' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$staging = Join-Path $OutputDirectory 'package'
New-Item -ItemType Directory -Path $staging | Out-Null
Copy-Item -LiteralPath $Executable -Destination "$staging/tshell.exe"
Copy-Item -LiteralPath $Executable -Destination "$OutputDirectory/tshell-windows-x86_64.exe"

# Include dependency notices without shipping repository paths or source files.
$metadata = cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata failed' }
$version = ($metadata.packages | Where-Object name -eq tshell).version
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Only stable releases are supported' }
$binary = Get-Item -LiteralPath "$OutputDirectory/tshell-windows-x86_64.exe"
if ($binary.Length -le 0 -or $binary.Length -gt 512MB) { throw 'Invalid binary size' }
[ordered]@{
    schema = 1
    version = $version
    target = 'x86_64-pc-windows-msvc'
    asset = $binary.Name
    size = $binary.Length
    sha256 = (Get-FileHash -LiteralPath $binary.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
} | ConvertTo-Json | Set-Content -LiteralPath "$OutputDirectory/update-windows-x86_64.json" -Encoding utf8NoBOM
$notices = [System.Collections.Generic.List[string]]::new()
$notices.Add('TShell third-party notices')
foreach ($package in ($metadata.packages | Sort-Object name, version)) {
    if ($package.name -eq 'tshell') { continue }
    $notices.Add("`n$($package.name) $($package.version) — $($package.license)")
    $directory = Split-Path $package.manifest_path -Parent
    $licenses = @(Get-ChildItem -LiteralPath $directory -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)([.-].*)?$' })
    foreach ($license in $licenses) {
        $notices.Add([System.IO.File]::ReadAllText($license.FullName))
    }
}
$notices.Add("`nBundled Microsoft ConPTY runtime")
$notices.Add([System.IO.File]::ReadAllText((Join-Path $PWD 'vendor/conpty/LICENSE')))
$notices | Set-Content -LiteralPath "$OutputDirectory/THIRD-PARTY-NOTICES.txt" -Encoding utf8
Copy-Item -LiteralPath "$OutputDirectory/THIRD-PARTY-NOTICES.txt" -Destination "$staging/THIRD-PARTY-NOTICES.txt"
@'
TShell for Windows x64

Extract this folder to a location you can write to, then run tshell.exe.
Application settings are kept separately in your user profile.
Official releases check for updates after launch and every six hours.
Updates install on the next launch; Settings > Updates also offers Restart and update.
Save files and close other TShell instances before updating.
Downloads: https://github.com/xxingwd/tshell/releases
'@ | Set-Content -LiteralPath "$staging/README.txt" -Encoding utf8
Compress-Archive -Path "$staging/*" -DestinationPath "$OutputDirectory/tshell-windows-x86_64.zip"
$assets = @(Get-ChildItem -LiteralPath $OutputDirectory -File | Where-Object Name -ne 'SHA256SUMS.txt' | Sort-Object Name)
$assets | ForEach-Object {
    "{0}  {1}" -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $_.Name
} | Set-Content -LiteralPath "$OutputDirectory/SHA256SUMS.txt" -Encoding ascii
