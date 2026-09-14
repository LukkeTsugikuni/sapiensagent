$ErrorActionPreference = 'Stop'

# Keep the Portuguese CMD messages readable on modern PowerShell and legacy
# consoles that inherit the process output encoding.
try {
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [Console]::OutputEncoding = $utf8
    $OutputEncoding = $utf8
} catch {
    # The agent itself remains usable if a host console does not expose this
    # encoding property.
}

$sapiensRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$sapiensBinary = Join-Path $sapiensRoot 'target\release\sapiens-agent.exe'

if (-not (Test-Path -LiteralPath $sapiensBinary)) {
    Write-Host 'Primeira execução: compilando o Sapiens Agent em modo release...'
    cargo build --release --manifest-path (Join-Path $sapiensRoot 'Cargo.toml')
}

Write-Host 'Sapiens Agent iniciando no CMD. Pressione Ctrl+C para encerrar.'
Write-Host 'WebUI local disponivel em http://127.0.0.1:8787/ (nao aberta automaticamente).'
& $sapiensBinary start
exit $LASTEXITCODE
