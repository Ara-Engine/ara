# Shared by engine registration and standalone registration. File operations are
# literal; the transaction lock is also used by setup and ara-hub-updater.
function New-AraHubShortcut([string]$Link, [string]$HubRoot) {
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Link)) | Out-Null
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($Link)
    $shortcut.TargetPath = Join-Path $HubRoot 'ara-hub.exe'
    $shortcut.WorkingDirectory = $HubRoot
    $shortcut.IconLocation = (Join-Path $HubRoot 'assets\brand\ara.ico') + ',0'
    $shortcut.Save()
}
function Register-AraHubShortcuts([string]$HubRoot, [string]$Programs) {
    $shortcuts = Join-Path $Programs 'Ara'
    New-AraHubShortcut (Join-Path $shortcuts 'Ara Hub.lnk') $HubRoot
    # Older versioned aliases must resolve to the same current launcher too.
    Get-ChildItem -LiteralPath $shortcuts -Filter 'Ara Hub.lnk' -Recurse -File | ForEach-Object {
        New-AraHubShortcut $_.FullName $HubRoot
    }
}

function Install-AraEngineHub([string]$Source, [string]$HubRoot) {
    $parent = [IO.Path]::GetDirectoryName($HubRoot)
    [IO.Directory]::CreateDirectory($parent) | Out-Null
    $lock = [IO.File]::Open((Join-Path $parent '.ara-hub-update.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::ReadWrite)
    $stage = Join-Path $parent ('.ara-hub-install-' + [guid]::NewGuid().ToString('N'))
    $backup = $HubRoot + '.old'
    $swapped = $false
    try {
        try { $lock.Lock(0,1) } catch { throw 'Another Hub update is running; retry when it finishes.' }
        if (Test-Path -LiteralPath $HubRoot) {
            if ((Test-Path -LiteralPath (Join-Path $HubRoot 'engine.json')) -or -not (Test-Path -LiteralPath (Join-Path $HubRoot 'hub.json'))) { throw 'Existing Ara Hub directory is unrelated.' }
            & (Join-Path $Source 'ara-hub-updater.exe') --is-newer (Join-Path $Source 'hub-launcher.json') (Join-Path $HubRoot 'hub.json') | Out-Host
            if ($LASTEXITCODE -eq 3) { return }
            if ($LASTEXITCODE -ne 0) { throw 'Could not compare Ara Hub build identities.' }
            $running = @(Get-Process -ErrorAction SilentlyContinue | ForEach-Object {
                try { $image = $_.Path } catch { $image = $null }
                if ($image -and (ConvertTo-AraPath $image).StartsWith($HubRoot + '\',[StringComparison]::OrdinalIgnoreCase)) { $_ }
            })
            if ($running.Count) { throw 'Close the Ara Hub window and retry installation; editors and the versioned Hub service can keep running.' }
            Get-ChildItem -LiteralPath $HubRoot -Recurse -File -Force | ForEach-Object {
                try { $probe = [IO.File]::Open($_.FullName,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::None); $probe.Dispose() }
                catch { throw 'Ara Hub files are in use; close the Hub and retry installation.' }
            }
        }
        if (Test-Path -LiteralPath $backup) { throw "Recover the previous Hub backup before retrying: $backup" }
        [IO.Directory]::CreateDirectory((Join-Path $stage 'bootstrap')) | Out-Null
        foreach ($file in @('ara-hub.exe','ara-hub-updater.exe','paths.ps1','hub-installation.ps1','THIRD_PARTY_NOTICES.txt')) {
            Copy-Item -LiteralPath (Join-Path $Source $file) -Destination (Join-Path $stage $file)
        }
        Copy-Item -LiteralPath (Join-Path $Source 'ara.exe') -Destination (Join-Path $stage 'bootstrap\ara.exe')
        Copy-Item -LiteralPath (Join-Path $Source 'hub-service.json') -Destination (Join-Path $stage 'bootstrap\service.json')
        Copy-Item -LiteralPath (Join-Path $Source 'standalone-hub-install.ps1') -Destination (Join-Path $stage 'install.ps1')
        Copy-Item -LiteralPath (Join-Path $Source 'hub-launcher.json') -Destination (Join-Path $stage 'hub.json')
        Get-ChildItem -LiteralPath $Source -Filter '*.dll' | ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination $stage
            Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $stage 'bootstrap')
        }
        [IO.Directory]::CreateDirectory((Join-Path $stage 'assets')) | Out-Null
        Copy-Item -LiteralPath (Join-Path $Source 'assets\brand') -Destination (Join-Path $stage 'assets') -Recurse
        if (Test-Path -LiteralPath $HubRoot) { Move-Item -LiteralPath $HubRoot -Destination $backup; $swapped = $true }
        try { Move-Item -LiteralPath $stage -Destination $HubRoot } catch {
            if ($swapped) { Move-Item -LiteralPath $backup -Destination $HubRoot; $swapped = $false }
            throw
        }
        if ($swapped) { Remove-Item -LiteralPath $backup -Recurse -Force }
    } finally {
        # Both paths are unique siblings we created, never user-supplied trees.
        if ((Test-Path -LiteralPath $stage) -and [IO.Path]::GetDirectoryName($stage) -eq $parent -and [IO.Path]::GetFileName($stage).StartsWith('.ara-hub-install-')) { Remove-Item -LiteralPath $stage -Recurse -Force }
        $lock.Dispose()
    }
}
