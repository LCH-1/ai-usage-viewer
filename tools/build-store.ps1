param(
    [ValidatePattern('^[1-9][0-9]*\.[0-9]+\.[0-9]+\.0$')]
    [string]$StoreVersion = "1.0.7.0"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.IO.Compression.FileSystem
Add-Type -AssemblyName System.Drawing

function Get-StreamSha256([System.IO.Stream]$Stream) {
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    try {
        return [BitConverter]::ToString($algorithm.ComputeHash($Stream)).Replace("-", "").ToLowerInvariant()
    } finally {
        $algorithm.Dispose()
    }
}

function Get-FileSha256([string]$Path) {
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        return Get-StreamSha256 $stream
    } finally {
        $stream.Dispose()
    }
}

function Assert-PackagedFile([System.IO.Compression.ZipArchive]$Archive, [string]$EntryName, [string]$Source) {
    $entry = $Archive.GetEntry($EntryName)
    if ($null -eq $entry) { throw "Package is missing $EntryName" }
    $stream = $entry.Open()
    try {
        if ((Get-StreamSha256 $stream) -ne (Get-FileSha256 $Source)) {
            throw "Package content differs from its source: $EntryName"
        }
    } finally {
        $stream.Dispose()
    }
}

$root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$runtimeSource = Join-Path $root "src-tauri\WebView2"
$runtimeExe = Join-Path $runtimeSource "msedgewebview2.exe"
if (-not (Test-Path -LiteralPath $runtimeExe -PathType Leaf)) {
    throw "Extract the official x64 WebView2 Fixed Version Runtime into src-tauri/WebView2 first."
}
$runtimeNotices = @(Get-ChildItem -LiteralPath $runtimeSource -File -Recurse -Force | Where-Object { $_.Name -match '(?i)license|third.?party.?notices|credits|copying|eula' })
if ($runtimeNotices.Count -eq 0) {
    throw "WebView2 redistribution license/notices are missing. Keep the complete official runtime contents."
}

$sdkRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
$sdk = Get-ChildItem -LiteralPath $sdkRoot -Directory |
    Where-Object { $_.Name -match '^10\.0\.\d+\.\d+$' } |
    Sort-Object { [version]$_.Name } -Descending |
    Where-Object { (Test-Path -LiteralPath (Join-Path $_.FullName "x64\makeappx.exe")) -and (Test-Path -LiteralPath (Join-Path $_.FullName "x64\makepri.exe")) } |
    Select-Object -First 1
if ($null -eq $sdk) { throw "Install the Windows SDK with x64 MakeAppx and MakePri tools."
}
$makeAppx = Join-Path $sdk.FullName "x64\makeappx.exe"
$makePri = Join-Path $sdk.FullName "x64\makepri.exe"
$appVersion = (Get-Content -LiteralPath (Join-Path $root "package.json") -Raw | ConvertFrom-Json).version
$packageVersion = [version]$StoreVersion
$artifactName = "AI-Usage-Viewer-Store-$($packageVersion.ToString(3))-x64.appx"
$release = Join-Path $root "release"
$finalArtifact = Join-Path $release $artifactName
if (Test-Path -LiteralPath $finalArtifact) { throw "Refusing to overwrite $finalArtifact. Choose a new StoreVersion."
}
$buildId = "{0}-{1}-{2}" -f $StoreVersion, (Get-Date -Format "yyyyMMdd-HHmmss"), [guid]::NewGuid().ToString("N")
$buildRoot = Join-Path $release "store-build\$buildId"
$layout = Join-Path $buildRoot "layout"
$assets = Join-Path $layout "assets"
$app = Join-Path $layout "app"
New-Item -ItemType Directory -Path $assets, $app | Out-Null

$originalPath = $env:PATH
Push-Location $root
try {
    $env:PATH = (Join-Path $env:USERPROFILE ".cargo\bin") + [System.IO.Path]::PathSeparator + $originalPath
    & corepack pnpm exec tauri build --no-bundle --config src-tauri/tauri.store.conf.json
    if ($LASTEXITCODE -ne 0) { throw "Store Tauri build failed with exit code $LASTEXITCODE."
    }
} finally {
    $env:PATH = $originalPath
    Pop-Location
}

$binary = Join-Path $root "src-tauri\target\release\ai-usage-viewer.exe"
if ([System.Diagnostics.FileVersionInfo]::GetVersionInfo($binary).ProductVersion -ne $appVersion) {
    throw "The built executable does not match package.json version $appVersion."
}
$iconNames = @("StoreLogo.png", "Square44x44Logo.png", "Square71x71Logo.png", "Square150x150Logo.png", "Square310x310Logo.png")
foreach ($iconName in $iconNames) {
    $iconSource = Join-Path $root "src-tauri\icons\$iconName"
    $bitmap = [System.Drawing.Bitmap]::new($iconSource)
    try {
        $colors = [System.Collections.Generic.HashSet[int]]::new()
        for ($y = 0; $y -lt $bitmap.Height; $y++) {
            for ($x = 0; $x -lt $bitmap.Width; $x++) {
                $pixel = $bitmap.GetPixel($x, $y)
                if ($pixel.A -gt 0) { [void]$colors.Add(($pixel.R -shl 16) -bor ($pixel.G -shl 8) -bor $pixel.B) }
            }
        }
        if ($colors.Count -lt 3) { throw "Tile $iconName is blank or contains a monochrome placeholder."
        }
    } finally {
        $bitmap.Dispose()
    }
    Copy-Item -LiteralPath $iconSource -Destination (Join-Path $assets $iconName)
}
Copy-Item -LiteralPath (Join-Path $assets "Square44x44Logo.png") -Destination (Join-Path $assets "Square44x44Logo.targetsize-44_altform-unplated.png")
$wideTile = [System.Drawing.Bitmap]::new(310, 150)
$tileGraphics = [System.Drawing.Graphics]::FromImage($wideTile)
$tileIcon = [System.Drawing.Bitmap]::new((Join-Path $assets "Square150x150Logo.png"))
try {
    $tileGraphics.Clear([System.Drawing.Color]::FromArgb(11, 15, 13))
    $tileGraphics.DrawImageUnscaled($tileIcon, 80, 0)
    $wideTile.Save((Join-Path $assets "Wide310x150Logo.png"), [System.Drawing.Imaging.ImageFormat]::Png)
} finally {
    $tileIcon.Dispose()
    $tileGraphics.Dispose()
    $wideTile.Dispose()
}
$iconNames += "Wide310x150Logo.png"

$manifestPath = Join-Path $layout "AppxManifest.xml"
[xml]$manifest = Get-Content -LiteralPath (Join-Path $root "build\store\AppxManifest.xml") -Raw
$manifest.Package.Identity.Version = $StoreVersion
$manifest.Save($manifestPath)
$priConfig = Join-Path $buildRoot "priconfig.xml"
& $makePri createconfig /cf $priConfig /dq ko-KR /pv 10.0.0 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "MakePri configuration failed with exit code $LASTEXITCODE."
}
& $makePri new /pr $layout /cf $priConfig /mn $manifestPath /of (Join-Path $layout "resources.pri") | Out-Null
if ($LASTEXITCODE -ne 0) { throw "MakePri resource indexing failed with exit code $LASTEXITCODE."
}

Copy-Item -LiteralPath $binary -Destination (Join-Path $app "ai-usage-viewer.exe")
Copy-Item -LiteralPath $runtimeSource -Destination (Join-Path $app "WebView2") -Recurse -Force
$artifact = Join-Path $buildRoot $artifactName
& $makeAppx pack /d $layout /p $artifact /h SHA256
if ($LASTEXITCODE -ne 0) { throw "MakeAppx schema/semantic validation or packaging failed with exit code $LASTEXITCODE."
}

$archive = [System.IO.Compression.ZipFile]::OpenRead($artifact)
try {
    Assert-PackagedFile $archive "app/ai-usage-viewer.exe" $binary
    Assert-PackagedFile $archive "app/WebView2/msedgewebview2.exe" $runtimeExe
    foreach ($iconName in $iconNames) {
        Assert-PackagedFile $archive "assets/$iconName" (Join-Path $assets $iconName)
    }
    Assert-PackagedFile $archive "assets/Square44x44Logo.targetsize-44_altform-unplated.png" (Join-Path $assets "Square44x44Logo.targetsize-44_altform-unplated.png")
    Assert-PackagedFile $archive "resources.pri" (Join-Path $layout "resources.pri")
    $manifestReader = [System.IO.StreamReader]::new($archive.GetEntry("AppxManifest.xml").Open())
    try { [xml]$packedManifest = $manifestReader.ReadToEnd() } finally { $manifestReader.Dispose() }
    if ($packedManifest.Package.Identity.Name -ne "LCH-1.AIUsageViewer" -or
        $packedManifest.Package.Identity.Publisher -ne "CN=3A4B4FB3-AF33-49D4-89DF-BE4A32F19AF3" -or
        $packedManifest.Package.Identity.Version -ne $StoreVersion -or
        $packedManifest.Package.Identity.ProcessorArchitecture -ne "x64" -or
        $packedManifest.Package.Applications.Application.Id -ne "AIUsageViewer" -or
        $packedManifest.Package.Applications.Application.Executable -ne "app\ai-usage-viewer.exe") {
        throw "Packaged manifest does not match the Microsoft Store identity or executable."
    }
    $runtimeFiles = @(Get-ChildItem -LiteralPath $runtimeSource -File -Recurse -Force)
    $packagedRuntimeFiles = @($archive.Entries | Where-Object { $_.FullName.StartsWith("app/WebView2/") -and -not $_.FullName.EndsWith("/") })
    if ($runtimeFiles.Count -ne $packagedRuntimeFiles.Count) { throw "Packaged WebView2 runtime is incomplete."
    }
} finally {
    $archive.Dispose()
}

Copy-Item -LiteralPath $artifact -Destination $finalArtifact
[pscustomobject]@{
    Artifact = $finalArtifact
    AppVersion = $appVersion
    StoreVersion = $StoreVersion
    WebView2Version = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($runtimeExe).ProductVersion
    Sha256 = Get-FileSha256 $finalArtifact
    BuildLayout = $layout
} | ConvertTo-Json
