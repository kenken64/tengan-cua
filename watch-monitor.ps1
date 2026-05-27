<#
.SYNOPSIS
Continuously monitors one Windows monitor with the Tengan CUA Codex workflow.

.EXAMPLE
.\watch-monitor.ps1 -Monitor 1

.EXAMPLE
.\watch-monitor.ps1 -Monitor 1 -IntervalSeconds 30 -Instruction "Watch for error dialogs and report them."

.EXAMPLE
.\watch-monitor.ps1 -Monitor 1 -Execute -Instruction "If a visible error dialog appears, click OK. Otherwise do nothing."

.EXAMPLE
.\watch-monitor.ps1 -Monitor 1 -Execute -TranscriptFile .\transcript.log
#>
[CmdletBinding()]
param(
    [int]$Monitor = 1,
    [int]$IntervalSeconds = 10,
    [string]$ContextFile = "$PSScriptRoot\context.txt",
    [string]$Instruction = "Inspect this monitor and summarize important changes. Return an empty actions array unless action is explicitly required.",
    [switch]$Execute,
    [switch]$Once,
    [string]$TranscriptFile = "",
    [string]$CodexBin = "codex.cmd"
)

$ErrorActionPreference = "Stop"

if (-not $PSScriptRoot) {
    $ProjectRoot = Get-Location
} else {
    $ProjectRoot = $PSScriptRoot
}

Set-Location $ProjectRoot

$TranscriptStarted = $false
if ($TranscriptFile) {
    $TranscriptPath = [System.IO.Path]::GetFullPath($TranscriptFile)
    $TranscriptDir = Split-Path -Parent $TranscriptPath

    if ($TranscriptDir -and -not (Test-Path -LiteralPath $TranscriptDir)) {
        New-Item -ItemType Directory -Path $TranscriptDir | Out-Null
    }

    Start-Transcript -Path $TranscriptPath -Append | Out-Null
    $TranscriptStarted = $true
}

if (-not (Test-Path -LiteralPath $ContextFile)) {
    @"
You are monitoring this Windows desktop screen.
Only report important changes.
Do not click, type, scroll, or move the mouse unless the user explicitly asks for an action.
Return an empty actions array unless an action is explicitly required.
"@ | Set-Content -LiteralPath $ContextFile -Encoding UTF8
}

try {
    Write-Host "Watching monitor $Monitor every $IntervalSeconds seconds." -ForegroundColor Cyan
    Write-Host "Context: $ContextFile" -ForegroundColor DarkCyan
    if ($TranscriptFile) {
        Write-Host "Transcript file: $TranscriptPath" -ForegroundColor DarkCyan
    }
    if (-not $Once) {
        Write-Host "Press Ctrl+C to stop." -ForegroundColor Yellow
    }

    do {
        $Context = Get-Content -LiteralPath $ContextFile -Raw
        $Prompt = "$Context`n`nCurrent task: $Instruction"
        $Timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"

        Write-Host ""
        Write-Host "[$Timestamp] Capturing monitor $Monitor..." -ForegroundColor Cyan

        $CargoArgs = @(
            "run",
            "--",
            "ask-codex",
            $Prompt,
            "--monitor",
            "$Monitor",
            "--codex-bin",
            $CodexBin
        )

        if ($Execute) {
            $CargoArgs += "--execute"
        }

        cargo @CargoArgs

        if ($LASTEXITCODE -ne 0) {
            Write-Warning "cargo exited with code $LASTEXITCODE"
        }

        if (-not $Once) {
            Start-Sleep -Seconds $IntervalSeconds
        }
    } while (-not $Once)
} finally {
    if ($TranscriptStarted) {
        Stop-Transcript | Out-Null
    }
}
