param(
    [string]$Coordinator = "$PSScriptRoot/../../../../.tools/p2p-config.dpapi",
    [string]$Distribution = 'Debian',
    [switch]$ReferenceWindows,
    [string]$Device,
    [ValidateRange(0, 20)]
    [int]$HoldSeconds = 0
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Security
$protected = [IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Coordinator))
$plain = [Security.Cryptography.ProtectedData]::Unprotect(
    $protected, $null, [Security.Cryptography.DataProtectionScope]::CurrentUser)
try {
    $json = [Text.Encoding]::UTF8.GetString($plain)
    if ($Device) {
        $devicePlain = [Security.Cryptography.ProtectedData]::Unprotect(
            [IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Device)), $null,
            [Security.Cryptography.DataProtectionScope]::CurrentUser)
        try {
            $profile = [Text.Encoding]::UTF8.GetString($devicePlain) | ConvertFrom-Json
            $config = $json | ConvertFrom-Json
            $config | Add-Member -NotePropertyName serial -NotePropertyValue $profile.serial
            $config | Add-Member -NotePropertyName username -NotePropertyValue $profile.username
            $config | Add-Member -NotePropertyName password -NotePropertyValue $profile.password
            if ($HoldSeconds) { $config | Add-Member -NotePropertyName hold_seconds -NotePropertyValue $HoldSeconds }
            $json = $config | ConvertTo-Json -Compress
        } finally { [Array]::Clear($devicePlain, 0, $devicePlain.Length) }
    }
    $root = (Resolve-Path "$PSScriptRoot/..").Path
    $linuxRoot = (& wsl -d $Distribution --exec wslpath -a $root.Replace('\', '/')).Trim()
    if ($LASTEXITCODE) { throw 'Não foi possível resolver o caminho no WSL.' }
    $start = [Diagnostics.ProcessStartInfo]::new('wsl.exe')
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.CreateNoWindow = $true
    $sdk = (Resolve-Path "$PSScriptRoot/../../../../sdk/NetSDK 3.050/Linux/bin").Path.Replace('\', '/')
    $linuxSdk = (& wsl -d $Distribution --exec wslpath -a $sdk).Trim()
    $arguments = @('-d', $Distribution, '--exec', 'env', "LD_LIBRARY_PATH=$linuxSdk", 'python3', "$linuxRoot/tests/probe.py")
    if ($ReferenceWindows) {
        $start.FileName = (Get-Command python3.exe).Source
        $arguments = @("$PSScriptRoot/reference_windows.py")
    }
    foreach ($argument in $arguments) {
        $start.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::Start($start)
    $process.StandardInput.Write($json)
    $process.StandardInput.Close()
    $process.WaitForExit()
    exit $process.ExitCode
} finally {
    [Array]::Clear($plain, 0, $plain.Length)
}
