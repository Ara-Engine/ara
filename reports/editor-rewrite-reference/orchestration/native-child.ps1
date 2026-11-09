param([Parameter(Mandatory=$true)][string]$LaunchConfig)
$ErrorActionPreference='Stop'
$configuration=Get-Content -Raw -Encoding UTF8 -LiteralPath $LaunchConfig | ConvertFrom-Json
if(!([IO.Path]::GetFullPath($configuration.localAppData).StartsWith('D:\AraBuild\ux-editor-target\',[StringComparison]::OrdinalIgnoreCase))){throw 'Editor child profile must remain within the task target'}
$env:LOCALAPPDATA=$configuration.localAppData
$arguments=@($configuration.arguments)
# Native stderr is preserved as evidence, and the native exit code remains authoritative.
$ErrorActionPreference='Continue'
& $configuration.executable @arguments
exit $LASTEXITCODE
