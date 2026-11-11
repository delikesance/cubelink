# Fabrique le paquet MSIX (Microsoft Store) à partir du build `tauri build --no-bundle`.
#   pwsh scripts/make-msix.ps1 -Version 0.1.8 -IdentityName <Partner Center> -Publisher "CN=..." -PublisherDisplayName "..."
# Le paquet n'est PAS signé : le Store le signe lui-même à la soumission.
param(
  [Parameter(Mandatory)][string]$Version,
  [string]$IdentityName = 'Cubelink.Placeholder',
  [string]$Publisher = 'CN=Placeholder',
  [string]$PublisherDisplayName = 'Cubelink',
  [string]$Out = 'target/release/bundle/msix'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$rel = Join-Path $root 'target/release'
$stage = Join-Path $root 'target/msix-stage'
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$stage/Assets" | Out-Null
New-Item -ItemType Directory -Force (Join-Path $root $Out) | Out-Null

Copy-Item "$rel/cubelink.exe" $stage
Copy-Item (Join-Path $root 'core/resources/cubelink-lan-agent.jar') $stage   # ressource Tauri : à côté de l'exe
foreach ($f in 'StoreLogo.png', 'Square150x150Logo.png', 'Square44x44Logo.png') {
  Copy-Item (Join-Path $root "src-tauri/icons/$f") "$stage/Assets/$f"
}
# MSIX veut MAJEUR.MINEUR.PATCH.0 (le dernier nombre est réservé au Store).
$manifest = Get-Content (Join-Path $root 'packaging/msix/AppxManifest.xml') -Raw -Encoding UTF8
$manifest = $manifest.Replace('@IDENTITY_NAME@', $IdentityName).Replace('@PUBLISHER@', $Publisher).Replace('@PUBLISHER_DISPLAY_NAME@', $PublisherDisplayName).Replace('@VERSION@', "$Version.0")
[IO.File]::WriteAllText("$stage/AppxManifest.xml", $manifest, (New-Object Text.UTF8Encoding $false))

$makeappx = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin\*\x64\makeappx.exe' | Sort-Object FullName | Select-Object -Last 1
if (-not $makeappx) { throw 'makeappx.exe introuvable (Windows SDK)' }
$pkg = Join-Path $root "$Out/Cubelink_${Version}_x64.msix"
& $makeappx.FullName pack /d $stage /p $pkg /o
if ($LASTEXITCODE -ne 0) { throw "makeappx a échoué ($LASTEXITCODE)" }
Write-Host "MSIX : $pkg"
