param()
$ErrorActionPreference = 'Stop'
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files\Eclipse Adoptium\jdk-25.0.2.10-hotspot\bin;$env:PATH"
$env:JAVA_HOME = 'C:\Program Files\Eclipse Adoptium\jdk-25.0.2.10-hotspot'
$runtime = Join-Path (Split-Path $PSScriptRoot -Parent) 'bridge/neoforge-runtime'
$cache = Join-Path $runtime '.cache'
$server = Join-Path $cache 'server'
$version = '26.2.0.93'
New-Item -ItemType Directory -Force $server | Out-Null

function Get-VerifiedDownload([string] $Url, [string] $Destination, [string] $Sha1) {
    if ((Test-Path -LiteralPath $Destination) -and
        (Get-FileHash -LiteralPath $Destination -Algorithm SHA1).Hash -eq $Sha1) { return }
    New-Item -ItemType Directory -Force (Split-Path $Destination -Parent) | Out-Null
    Invoke-WebRequest $Url -OutFile "$Destination.part"
    if ((Get-FileHash -LiteralPath "$Destination.part" -Algorithm SHA1).Hash -ne $Sha1) {
        throw "SHA1 mismatch downloading $Url"
    }
    Move-Item -LiteralPath "$Destination.part" -Destination $Destination -Force
}

$manifest = Invoke-RestMethod 'https://piston-meta.mojang.com/mc/game/version_manifest_v2.json'
$release = $manifest.versions | Where-Object id -EQ '26.2' | Select-Object -First 1
if (!$release) { throw 'Minecraft 26.2 is missing from the Mojang manifest' }
$versionJson = Join-Path $cache 'minecraft-version.json'
Get-VerifiedDownload $release.url $versionJson $release.sha1
$minecraft = Get-Content -LiteralPath $versionJson -Raw | ConvertFrom-Json
$bundler = Join-Path $server 'libraries/net/minecraft/server/26.2/server-26.2.jar'
Get-VerifiedDownload $minecraft.downloads.server.url $bundler $minecraft.downloads.server.sha1
$installerUrl = "https://maven.neoforged.net/releases/net/neoforged/neoforge/$version/neoforge-$version-installer.jar"
$installer = Join-Path $cache 'neoforge-installer.jar'
$shaResponse = (Invoke-WebRequest "$installerUrl.sha1").Content
$installerSha1 = if ($shaResponse -is [byte[]]) { [Text.Encoding]::ASCII.GetString($shaResponse).Trim() } else { $shaResponse.Trim() }
Get-VerifiedDownload $installerUrl $installer $installerSha1
$installed = Join-Path $server "libraries/net/neoforged/neoforge/$version/win_args.txt"
$patched = Join-Path $server "libraries/net/neoforged/minecraft-server-patched/$version/minecraft-server-patched-$version.jar"
$universal = Join-Path $server "libraries/net/neoforged/neoforge/$version/neoforge-$version-universal.jar"
$complete = Join-Path $cache 'install-complete'
$needsInstall = !(Test-Path -LiteralPath $installed) -or !(Test-Path -LiteralPath $patched) -or
    !(Test-Path -LiteralPath $universal) -or !(Test-Path -LiteralPath $complete)
if (!$needsInstall) {
    $lines = Get-Content -LiteralPath $installed
    $classPathLine = [Array]::IndexOf($lines, '-classpath') + 1
    foreach ($library in $lines[$classPathLine].Split(';')) {
        if (!(Test-Path -LiteralPath (Join-Path $server $library))) { $needsInstall = $true; break }
    }
}
if ($needsInstall) {
    Push-Location $server
    try {
        & "$env:JAVA_HOME/bin/java.exe" -jar $installer --installServer . *> (Join-Path $cache 'install.log')
        if ($LASTEXITCODE -ne 0) { throw "NeoForge installer failed; see $cache/install.log" }
        Set-Content -LiteralPath $complete -Value $installerSha1 -Encoding ascii
    } finally { Pop-Location }
}
$lines = Get-Content -LiteralPath $installed
$mainIndex = [Array]::IndexOf($lines, 'net.neoforged.fml.startup.Server')
$options = [Collections.Generic.List[string]]::new()
for ($index = 0; $index -lt $mainIndex; $index++) {
    $line = $lines[$index]
    if ($line -eq '-classpath') {
        $index++
        $classPath = @($lines[$index].Split(';') | ForEach-Object { (Join-Path $server $_).Replace('\', '/') })
        $classPath += (Join-Path $runtime 'build/libs/ferrum-neoforge-runtime-1.0.0-bootstrap.jar').Replace('\', '/')
        $options.Add('-Djava.class.path=' + ($classPath -join ';'))
    } elseif ($line.StartsWith('-DlibraryDirectory=')) {
        $options.Add('-DlibraryDirectory=' + (Join-Path $server 'libraries').Replace('\', '/'))
    } else {
        $options.Add(($line -replace '^(--add-opens|--add-exports) ', '$1='))
    }
}
$options.Add('-Xmx4G')
$options.Add('--enable-native-access=ALL-UNNAMED')
$options.Add('--sun-misc-unsafe-memory-access=allow')
@{ entryClass = 'net.solarspire.ferrum.runtime.BridgeRuntime'; vmOptions = $options.ToArray() } |
    ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $cache 'jvm.json') -Encoding utf8NoBOM
Write-Host "Prepared Minecraft 26.2 / NeoForge $version in $server"
