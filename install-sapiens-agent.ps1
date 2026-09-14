param(
    [switch]$SkipBuild,
    [switch]$DryRun,
    [switch]$Rebuild
)

$ErrorActionPreference = 'Stop'
$sapiensRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$sapiensBinary = Join-Path $sapiensRoot 'target\release\sapiens-agent.exe'
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$pathEntries = @($userPath -split ';' | Where-Object { $_ -and $_.Trim() })
$alreadyInstalled = $pathEntries | Where-Object { [StringComparer]::OrdinalIgnoreCase.Equals($_.TrimEnd('\'), $sapiensRoot.TrimEnd('\')) }

if (-not $SkipBuild -and ($Rebuild -or -not (Test-Path -LiteralPath $sapiensBinary))) {
    if ($DryRun) {
        Write-Host "[dry-run] cargo build --release --manifest-path $sapiensRoot\Cargo.toml"
    } else {
        $backup = "$sapiensBinary.previous"
        $hadBinary = Test-Path -LiteralPath $sapiensBinary
        if ($hadBinary) {
            Copy-Item -LiteralPath $sapiensBinary -Destination $backup -Force
        }
        try {
            & cargo build --release --manifest-path (Join-Path $sapiensRoot 'Cargo.toml')
            if ($LASTEXITCODE -ne 0) {
                throw "cargo build failed with exit code $LASTEXITCODE"
            }
            if (-not (Test-Path -LiteralPath $sapiensBinary)) {
                throw 'release binary was not produced'
            }
            $length = (Get-Item -LiteralPath $sapiensBinary).Length
            if ($length -le 0) {
                throw 'release binary is empty'
            }
            if (Test-Path -LiteralPath $backup) {
                Remove-Item -LiteralPath $backup -Force
            }
        } catch {
            if (Test-Path -LiteralPath $backup) {
                Copy-Item -LiteralPath $backup -Destination $sapiensBinary -Force
            }
            throw
        }
    }
}

if (Test-Path -LiteralPath $sapiensBinary) {
    $hash = (Get-FileHash -LiteralPath $sapiensBinary -Algorithm SHA256).Hash
    Write-Host "Release SHA-256: $hash"
}

if (-not $alreadyInstalled) {
    if ($DryRun) {
        Write-Host "[dry-run] adicionar ao PATH do usuario: $sapiensRoot"
    } else {
        $newPath = (($pathEntries + $sapiensRoot) -join ';')
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        $env:Path = "$sapiensRoot;$env:Path"
    }
}

if ($DryRun) {
    Write-Host 'Dry-run concluido; nenhuma alteracao foi aplicada.'
} else {
    Write-Host 'Sapiens Agent instalado.'
}
Write-Host 'Abra um novo CMD e use: sapiens-agent ou sapiens'
Write-Host 'Inicio: sapiens-agent start   |   Configuracao: sapiens-agent setup'
