# Stick 'Em Up 2 (Rust remake) setup.
#
# The remake ships no game files. This script finds your own copy of Stick 'Em Up 2 in your
# Steam library and exports its data (art, sounds, levels) with AssetRipper into
# assets_exported\ExportedProject next to this script, where the launcher and the game find it.
#
# AssetRipper (GPL-3.0, https://github.com/AssetRipper/AssetRipper) is downloaded from its
# GitHub releases into tools\AssetRipper the first time; it is not part of the remake.
#
# Run it again with -Force to export again.

param([switch]$Force)

$ErrorActionPreference = 'Stop'
$AppId = 282780
$StoreUrl = "https://store.steampowered.com/app/$AppId"
$AssetRipperVersion = '2.0.0'
$Port = 53817
$Root = $PSScriptRoot
$OutDir = Join-Path $Root 'assets_exported'
$Exported = Join-Path $OutDir 'ExportedProject'
$Check = Join-Path $Exported 'Assets\Game\Lobby\Lobby.unity'

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Say($text) { Write-Host $text }
function SteamTip {
    # Steam can open the launcher when Play is pressed: its launch options run the launcher with
    # the original's command (%command%), which the launcher's "Play Original" button uses.
    $launcher = Join-Path $Root 'sticks_launcher.exe'
    if (-not (Test-Path $launcher)) { $launcher = Join-Path $Root 'target\release\sticks_launcher.exe' }
    Say ''
    Say "To open it from Steam: right-click Stick 'Em Up 2 in your library, Properties,"
    Say 'and paste this into Launch Options (clear the box to undo):'
    Write-Host ('"' + $launcher + '" %command%') -ForegroundColor Cyan
}
function Fail($text) { Write-Host ''; Write-Host $text -ForegroundColor Red; exit 1 }

Say "Stick 'Em Up 2 (Rust remake) setup"
Say '-----------------------------------'

if ((Test-Path $Check) -and -not $Force) {
    Say 'The game data is already exported. You can start sticks_launcher.exe.'
    Say '(Run Setup.bat -Force to export it again.)'
    SteamTip
    exit 0
}

# 1. Steam and its libraries.
$steam = $null
foreach ($key in 'HKCU:\Software\Valve\Steam', 'HKLM:\SOFTWARE\WOW6432Node\Valve\Steam', 'HKLM:\SOFTWARE\Valve\Steam') {
    $p = Get-ItemProperty -Path $key -ErrorAction SilentlyContinue
    if ($p) {
        foreach ($name in 'SteamPath', 'InstallPath') {
            $v = $p.$name
            if ($v -and (Test-Path $v)) { $steam = $v -replace '/', '\'; break }
        }
    }
    if ($steam) { break }
}
if (-not $steam) {
    $default = 'C:\Program Files (x86)\Steam'
    if (Test-Path $default) { $steam = $default }
}
if (-not $steam) {
    Fail "Steam is not installed. Stick 'Em Up 2 is a Steam game: install Steam (https://store.steampowered.com/about/), buy the game ($StoreUrl), install it, then run Setup.bat again."
}
Say "Steam: $steam"

$libraries = @($steam)
$vdf = Join-Path $steam 'steamapps\libraryfolders.vdf'
if (Test-Path $vdf) {
    foreach ($m in [regex]::Matches((Get-Content $vdf -Raw), '"path"\s+"([^"]+)"')) {
        $libraries += ($m.Groups[1].Value -replace '\\\\', '\')
    }
}
$libraries = $libraries | Select-Object -Unique

# 2. The game in one of them.
$game = $null
foreach ($lib in $libraries) {
    $manifest = Join-Path $lib "steamapps\appmanifest_$AppId.acf"
    $installDir = "Stick 'Em Up 2"
    if (Test-Path $manifest) {
        $m = [regex]::Match((Get-Content $manifest -Raw), '"installdir"\s+"([^"]+)"')
        if ($m.Success) { $installDir = $m.Groups[1].Value }
    }
    $dir = Join-Path $lib "steamapps\common\$installDir"
    if ((Test-Path $dir) -and (Get-ChildItem -Path $dir -Directory -Filter '*_Data' -ErrorAction SilentlyContinue)) {
        $game = $dir
        break
    }
}
if (-not $game) {
    Say ''
    Say "Stick 'Em Up 2 is not installed in your Steam library."
    Say "If you own it, install it in Steam. If you don't, it is on the Steam store: $StoreUrl"
    $answer = Read-Host 'Open Steam to install (or buy) it now? [Y/n]'
    if ($answer -notmatch '^[Nn]') { Start-Process "steam://install/$AppId" }
    Fail 'Run Setup.bat again once the game is installed.'
}
Say "Game: $game"

# 3. AssetRipper.
$tools = Join-Path $Root 'tools\AssetRipper'
$exe = Get-ChildItem -Path $tools -Recurse -Filter 'AssetRipper.GUI.Free.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $exe) {
    Say "Downloading AssetRipper $AssetRipperVersion (GPL-3.0) from GitHub..."
    $release = Invoke-RestMethod -UseBasicParsing -Uri "https://api.github.com/repos/AssetRipper/AssetRipper/releases/tags/$AssetRipperVersion" -Headers @{ 'User-Agent' = 'StickEmUp2-remake-setup' }
    $asset = $release.assets | Where-Object { $_.name -match 'win' -and $_.name -match 'x64' -and $_.name -match '\.zip$' } | Select-Object -First 1
    if (-not $asset) { Fail "No Windows x64 download in AssetRipper $AssetRipperVersion. Download it yourself from https://github.com/AssetRipper/AssetRipper/releases and unzip it into $tools" }
    New-Item -ItemType Directory -Force -Path $tools | Out-Null
    $zip = Join-Path $tools $asset.name
    Invoke-WebRequest -UseBasicParsing -Uri $asset.browser_download_url -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $tools -Force
    Remove-Item $zip
    $exe = Get-ChildItem -Path $tools -Recurse -Filter 'AssetRipper.GUI.Free.exe' | Select-Object -First 1
    if (-not $exe) { Fail "AssetRipper.GUI.Free.exe was not in the download (see $tools)." }
}

# 4. Export, with AssetRipper running headless (its local web interface on $Port).
Say 'Starting AssetRipper...'
$ripper = Start-Process -FilePath $exe.FullName -ArgumentList '--headless', '--port', "$Port" -WorkingDirectory $exe.DirectoryName -WindowStyle Hidden -PassThru
$base = "http://127.0.0.1:$Port"
$problem = $null
try {
    $up = $false
    for ($i = 0; $i -lt 60 -and -not $up; $i++) {
        Start-Sleep -Seconds 1
        try { Invoke-WebRequest -UseBasicParsing -Uri "$base/" -TimeoutSec 2 | Out-Null; $up = $true } catch { }
    }
    if (-not $up) { throw 'AssetRipper did not start (see its log in tools\AssetRipper).' }

    # The remake reads the game's scripts as C# files (their .meta files name the classes).
    try { Invoke-WebRequest -UseBasicParsing -Method Post -Uri "$base/Settings/Update" -Body @{ ScriptExportMode = 'Decompiled' } -TimeoutSec 30 | Out-Null } catch { Say "(AssetRipper settings were not changed: $($_.Exception.Message))" }

    Say 'Reading the game (this takes a minute)...'
    Invoke-WebRequest -UseBasicParsing -Method Post -Uri "$base/LoadFolder" -Body @{ Path = $game } -TimeoutSec 0 | Out-Null

    if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    Say 'Exporting the art, sounds and levels (this takes a few minutes)...'
    Invoke-WebRequest -UseBasicParsing -Method Post -Uri "$base/Export/UnityProject" -Body @{ Path = $OutDir } -TimeoutSec 0 | Out-Null
}
catch {
    $problem = $_.Exception.Message
}
finally {
    if ($ripper -and -not $ripper.HasExited) { Stop-Process -Id $ripper.Id -Force -ErrorAction SilentlyContinue }
}
if ($problem) { Fail "The export failed: $problem" }

# 5. Where the project landed: it must be assets_exported\ExportedProject (AssetRipper versions
#    differ in whether they add the ExportedProject folder themselves).
if (-not (Test-Path $Check)) {
    $lobby = Get-ChildItem -Path $OutDir -Recurse -Filter 'Lobby.unity' -ErrorAction SilentlyContinue | Where-Object { $_.FullName -like '*\Assets\Game\Lobby\Lobby.unity' } | Select-Object -First 1
    if ($lobby) {
        $project = $lobby.Directory.Parent.Parent.Parent.FullName
        if ($project -eq (Resolve-Path $OutDir).Path) {
            $moved = "$OutDir-moving"
            Rename-Item -Path $OutDir -NewName (Split-Path $moved -Leaf)
            New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
            Move-Item -Path $moved -Destination $Exported
        } elseif ($project -ne $Exported) {
            Move-Item -Path $project -Destination $Exported
        }
    }
}

# 6. Check.
if (-not (Test-Path $Check)) {
    Fail "The export finished but $Check is missing. The remake expects AssetRipper's Unity project layout (assets_exported\ExportedProject\Assets\...)."
}
if (-not (Test-Path (Join-Path $Exported 'Assets\Scripts\Assembly-CSharp'))) {
    Write-Host 'Warning: the game scripts were not exported as C# files (Assets\Scripts\Assembly-CSharp); some things may not load.' -ForegroundColor Yellow
}
Say ''
Write-Host 'Done. Start sticks_launcher.exe to play.' -ForegroundColor Green
SteamTip
