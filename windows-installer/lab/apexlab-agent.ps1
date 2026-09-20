# ─────────────────────────────────────────────────────────────────────────────
#  apexlab-agent.ps1 — the guest half of the lab, and the only thing baked into
#  the golden image.
#
#  It runs from an ON-START scheduled task as SYSTEM, so there is no logon
#  session, no autologon and no interactive desktop anywhere in the loop. That
#  matters: every one of those is a thing that can fail to happen on a headless
#  guest and leave the harness waiting on a timeout with nothing to read.
#
#  The contract with the host is one FAT volume labelled APEXLAB:
#
#      run.ps1      the host puts the work here; absent means "just boot"
#      result.txt   the guest writes everything here, host reads it after
#      status.txt   a single word, so a truncated result.txt is not mistaken
#                   for a pass
#
#  If there is no APEXLAB volume the agent exits and leaves the machine up.
#  That is the maintenance path: the golden image has to remain bootable by
#  hand, and an agent that powers the machine off unconditionally makes that
#  impossible.
# ─────────────────────────────────────────────────────────────────────────────
$ErrorActionPreference = 'Continue'

function Find-LabVolume {
    # Volumes are not necessarily mounted when an on-start task fires. Poll
    # rather than assume; a missing drive letter here reads exactly like "the
    # host attached no transport disk", and those are different situations.
    for ($i = 0; $i -lt 120; $i++) {
        $v = Get-Volume -ErrorAction SilentlyContinue |
             Where-Object { $_.FileSystemLabel -eq 'APEXLAB' -and $_.DriveLetter }
        if ($v) { return ($v | Select-Object -First 1) }
        Start-Sleep -Seconds 1
    }
    return $null
}

$vol = Find-LabVolume
if (-not $vol) { exit 0 }

$root   = "$($vol.DriveLetter):"
$result = Join-Path $root 'result.txt'
$status = Join-Path $root 'status.txt'
$runner = Join-Path $root 'run.ps1'

"APEXLAB-AGENT-START $(Get-Date -Format o)"      | Out-File -FilePath $result -Encoding ascii
"computer: $env:COMPUTERNAME"                     | Out-File -FilePath $result -Encoding ascii -Append
"identity: $([Security.Principal.WindowsIdentity]::GetCurrent().Name)" |
    Out-File -FilePath $result -Encoding ascii -Append
"os      : $((Get-CimInstance Win32_OperatingSystem).Caption)" |
    Out-File -FilePath $result -Encoding ascii -Append

if (Test-Path $runner) {
    try {
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $runner 2>&1 |
            Out-File -FilePath $result -Encoding ascii -Append
        $rc = $LASTEXITCODE
    } catch {
        "AGENT-EXCEPTION: $_" | Out-File -FilePath $result -Encoding ascii -Append
        $rc = 199
    }
    "APEXLAB-RUN-EXIT $rc" | Out-File -FilePath $result -Encoding ascii -Append
    if ($rc -eq 0) { 'PASS' | Out-File -FilePath $status -Encoding ascii }
    else           { "FAIL $rc" | Out-File -FilePath $status -Encoding ascii }
} else {
    'NORUNNER' | Out-File -FilePath $status -Encoding ascii
}

"APEXLAB-AGENT-END $(Get-Date -Format o)" | Out-File -FilePath $result -Encoding ascii -Append

# Flush before the power goes. A FAT volume the host is about to read with
# mtools does not forgive a lazy writer.
[System.IO.File]::AppendAllText($result, "")
Start-Sleep -Seconds 2
Stop-Computer -Force
