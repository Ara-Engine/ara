# Windows PowerShell 5.1 providers reject Win32 verbatim prefixes. Convert the
# representable drive/UNC forms before using Join-Path or other provider cmdlets.
function ConvertTo-AraPath([string]$Path) {
    if ($Path.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) {
        $Path = '\\' + $Path.Substring(8)
    } elseif ($Path.StartsWith('\\?\', [StringComparison]::OrdinalIgnoreCase) -and
              $Path.Length -ge 7 -and $Path[5] -eq ':' -and $Path[6] -eq '\') {
        $Path = $Path.Substring(4)
    }
    [IO.Path]::GetFullPath($Path)
}
