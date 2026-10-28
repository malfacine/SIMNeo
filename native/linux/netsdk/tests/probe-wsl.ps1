param(
    [Parameter(Mandatory)][string]$Device,
    [ValidateSet(0,19)][int]$Mode = 19,
    [string]$HostAddress,
    [string]$Distribution = 'Debian'
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Security
$bytes = [Security.Cryptography.ProtectedData]::Unprotect(
    [IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Device)), $null,
    [Security.Cryptography.DataProtectionScope]::CurrentUser)
try {
    $json = [Text.Encoding]::UTF8.GetString($bytes)
    if ($HostAddress) {
        $profile = $json | ConvertFrom-Json
        $profile | Add-Member -NotePropertyName host -NotePropertyValue $HostAddress
        $json = $profile | ConvertTo-Json -Compress
    }
    $root = (Resolve-Path "$PSScriptRoot/..").Path.Replace('\', '/')
    $sdk = (Resolve-Path "$PSScriptRoot/../../../../sdk/NetSDK 3.050/Linux/bin").Path.Replace('\', '/')
    $linuxRoot = (& wsl -d $Distribution --exec wslpath -a $root).Trim()
    $linuxSdk = (& wsl -d $Distribution --exec wslpath -a $sdk).Trim()
    $start = [Diagnostics.ProcessStartInfo]::new('wsl.exe')
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.CreateNoWindow = $true
    foreach ($argument in @('-d', $Distribution, '--exec', 'env', "LD_LIBRARY_PATH=$linuxSdk",
                             'python3', "$linuxRoot/tests/probe.py", [string]$Mode)) {
        $start.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::Start($start)
    $process.StandardInput.Write($json)
    $process.StandardInput.Close()
    $process.WaitForExit()
    exit $process.ExitCode
} finally { [Array]::Clear($bytes, 0, $bytes.Length) }
