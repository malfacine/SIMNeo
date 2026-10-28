param(
    [string]$Device = "$PSScriptRoot/../../.tools/test-device.dpapi",
    [string]$Distribution = 'Debian',
    [string]$BinaryDirectory = '/home/doc/.cache/simneo-target/debug',
    [ValidateSet('all', 'video', 'mosaic', 'playback')]
    [string]$Mode = 'all',
    [string]$AppImage
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Security
$plain = [Security.Cryptography.ProtectedData]::Unprotect(
    [IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Device)), $null,
    [Security.Cryptography.DataProtectionScope]::CurrentUser)
try {
    $script = (Resolve-Path "$PSScriptRoot/test-ui.sh").Path.Replace('\', '/')
    $linuxScript = (& wsl -d $Distribution --exec wslpath -a $script).Trim()
    if ($LASTEXITCODE) { throw 'Não foi possível resolver o caminho no WSL.' }
    $linuxImage = $null
    if ($AppImage) {
        $imagePath = (Resolve-Path -LiteralPath $AppImage).Path.Replace('\', '/')
        $linuxImage = (& wsl -d $Distribution --exec wslpath -a $imagePath).Trim()
        if ($LASTEXITCODE) { throw 'Não foi possível resolver o AppImage no WSL.' }
    }
    $start = [Diagnostics.ProcessStartInfo]::new('wsl.exe')
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.CreateNoWindow = $true
    $arguments = @('-d', $Distribution, '--exec', 'sh', $linuxScript, $BinaryDirectory, $Mode)
    if ($linuxImage) { $arguments += $linuxImage }
    # .NET Framework do PowerShell 5.1 não oferece ArgumentList. Arguments é
    # compatível com ambas as versões; escapar cada argumento preserva espaços,
    # aspas e barras finais sem passar por um shell intermediário.
    $start.Arguments = ($arguments | ForEach-Object {
        if ($_.Length -gt 0 -and $_ -notmatch '[\s"]') {
            $_
        } else {
            $quoted = [regex]::Replace($_, '(\\*)"', '$1$1\"')
            $quoted = [regex]::Replace($quoted, '(\\+)$', '$1$1')
            '"' + $quoted + '"'
        }
    }) -join ' '
    $process = [Diagnostics.Process]::Start($start)
    # Enviar os bytes UTF-8 originais também no PowerShell 5.1, sem conversão
    # para a página de códigos do console e sem credenciais em argumentos.
    $process.StandardInput.BaseStream.Write($plain, 0, $plain.Length)
    $process.StandardInput.Close()
    $process.WaitForExit()
    if ($process.ExitCode) { throw "Teste da interface Linux falhou ($($process.ExitCode))." }
} finally { [Array]::Clear($plain, 0, $plain.Length) }
