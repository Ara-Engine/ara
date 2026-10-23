$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
$picker = New-Object System.Windows.Forms.FolderBrowserDialog
$picker.Description = 'Select an Ara project folder (contains ara.project.json)'
if ($picker.ShowDialog() -eq 'OK') {
    & (Join-Path $PSScriptRoot 'ara-editor.exe') $picker.SelectedPath
    if ($LASTEXITCODE -ne 0) { Read-Host 'Editor failed; see the diagnostic above. Press Enter to close' }
}
