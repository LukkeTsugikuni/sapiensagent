param(
    [switch]$SkipBuild,
    [switch]$DryRun,
    [switch]$Rebuild
)

$ErrorActionPreference = 'Stop'
$sapiensRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$sapiensBinary = Join-Path $sapiensRoot 'target\release\sapiens-agent.exe'
$userBin = Join-Path $env:LOCALAPPDATA 'SapiensAgent\bin'
$installedBinary = Join-Path $userBin 'sapiens-agent-runtime.exe'
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$pathEntries = @($userPath -split ';' | Where-Object { $_ -and $_.Trim() })
$pathTargets = @($userBin, $sapiensRoot)
$managedPathEntries = @($pathEntries | Where-Object {
    $entry = $_.TrimEnd('\')
    -not ($pathTargets | Where-Object {
        [StringComparer]::OrdinalIgnoreCase.Equals($entry, $_.TrimEnd('\'))
    })
})
$desiredPathEntries = @($userBin) + $managedPathEntries + @($sapiensRoot)
$pathNeedsUpdate = (($pathEntries -join ';') -ne ($desiredPathEntries -join ';'))

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
} elseif (-not $DryRun) {
    throw "release binary not found: $sapiensBinary. Run without -SkipBuild to compile it."
}

if ($DryRun) {
    Write-Host "[dry-run] criar lancadores globais em: $userBin"
    Write-Host "[dry-run] priorizar no PATH do usuario: $userBin"
} else {
    New-Item -ItemType Directory -Force -Path $userBin | Out-Null
    Copy-Item -LiteralPath $sapiensBinary -Destination $installedBinary -Force
    $launcher = "@echo off`r`n`"$installedBinary`" %*`r`n"
    foreach ($name in @('sapiens.cmd', 'sapiens-agent.cmd')) {
        Set-Content -LiteralPath (Join-Path $userBin $name) -Value $launcher -Encoding ascii
    }
}

if ($pathNeedsUpdate) {
    if ($DryRun) {
        Write-Host "[dry-run] PATH sera atualizado para o proximo PowerShell"
    } else {
        $newPath = ($desiredPathEntries -join ';')
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        $env:Path = (($userBin, $sapiensRoot -join ';') + ";$env:Path")
    }
}

if ($DryRun) {
    Write-Host 'Dry-run concluido; nenhuma alteracao foi aplicada.'
} else {
    Write-Host 'Sapiens Agent instalado.'
}
Write-Host 'Feche este terminal, abra um novo PowerShell e use: sapiens'
Write-Host 'Inicio direto: sapiens start   |   Configuracao: sapiens setup'
