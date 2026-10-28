param([string]$Distribution = 'Debian')
$ErrorActionPreference = 'Stop'
# Executa os trechos reais do runner com dados sintéticos. Não lê perfis DPAPI
# nem abre conexão com o DVR; verifica a fronteira PowerShell -> WSL.
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    "$PSScriptRoot/test-ui-wsl.ps1", [ref]$tokens, [ref]$parseErrors)
if ($parseErrors) { throw $parseErrors }
$assignment = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
        $node.Left.Extent.Text -eq '$start.Arguments'
}, $true)
$write = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.InvokeMemberExpressionAst] -and
        $node.Extent.Text -match '^\$process\.StandardInput(?:\.BaseStream)?\.Write\('
}, $true)
if (!$assignment -or !$write) { throw 'Trechos de envio do runner não encontrados.' }
$serialize = [scriptblock]::Create($assignment.Extent.Text)
$send = [scriptblock]::Create($write.Extent.Text)
foreach ($case in @('arguments', 'stdin')) {
    $start = [Diagnostics.ProcessStartInfo]::new('wsl.exe')
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = [Text.Encoding]::UTF8
    $values = @('with spaces', '', 'São Paulo', 'a"b', 'trailing\', 'space trailing\', '$(echo unsafe)', '; echo unsafe')
    if ($case -eq 'arguments') {
        $arguments = @('-d', $Distribution, '--exec', '/usr/bin/printf', '%s\n') + $values
        $expected = ($values -join "`n") + "`n"
    } else {
        $arguments = @('-d', $Distribution, '--exec', '/usr/bin/cat')
        $expected = 'UTF-8 sintético: usuário, ação, São Paulo.'
    }
    . $serialize
    $process = [Diagnostics.Process]::Start($start)
    try {
        if ($case -eq 'stdin') {
            $plain = [Text.Encoding]::UTF8.GetBytes($expected)
            . $send
        }
        $process.StandardInput.Close()
        $actual = $process.StandardOutput.ReadToEnd()
        $failure = $process.StandardError.ReadToEnd()
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) { throw "WSL falhou no teste $case ($($process.ExitCode)): $failure" }
        if ($actual -cne $expected) { throw "O teste $case alterou os dados enviados ao WSL." }
        Write-Output "PowerShell $($PSVersionTable.PSVersion): $case preservado."
    } finally {
        if ($plain) { [Array]::Clear($plain, 0, $plain.Length) }
        $process.Dispose()
    }
}
