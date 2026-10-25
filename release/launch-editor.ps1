$ErrorActionPreference = 'Stop'
. ([IO.Path]::Combine($PSScriptRoot, 'paths.ps1'))
$source = ConvertTo-AraPath $PSScriptRoot
Add-Type -AssemblyName System.Windows.Forms
$picker = New-Object System.Windows.Forms.FolderBrowserDialog
$picker.Description = 'Select an Ara project folder (contains ara.project.json)'
if ($picker.ShowDialog() -eq 'OK') {
    & (Join-Path $source 'ara-editor.exe') (ConvertTo-AraPath $picker.SelectedPath)
    if ($LASTEXITCODE -ne 0) { Read-Host 'Editor failed; see the diagnostic above. Press Enter to close' }
}
