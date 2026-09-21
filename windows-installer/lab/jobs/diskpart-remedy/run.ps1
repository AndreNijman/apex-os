# ─────────────────────────────────────────────────────────────────────────────
#  jobs/diskpart-remedy — does the "set id=" remedy plan.rs hands the user for
#  a Windows basic-data partition actually get past plan::assess, or does it
#  walk into a second refusal?
#
#  This was never exercised. Two separate things could stop it, and the order
#  they are checked in matters because assess() checks OWNERSHIP before it
#  checks the GPT ATTRIBUTES:
#
#    1. Windows auto-mounts a raw (unformatted) basic-data partition the
#       moment it sees the type GUID -- fixture-a's "Blank basic" partition
#       reads "REFUSED (in use by Windows) ... mounted at F:\ -- unrecognised
#       filesystem" today, not the basic-data-type refusal the remedy text
#       hangs off. So the FIRST question is whether retyping the partition
#       makes Windows drop the volume object it already created, since
#       ARCHITECTURE.md's "no volume for a Linux-filesystem partition" claim
#       was only ever measured on a partition that was Linux-typed from
#       creation, never on one retyped while a volume already existed on it.
#    2. Every Windows-made partition on the golden image carries GPT attribute
#       bit 63 (0x8000000000000000, Microsoft's GPT_BASIC_DATA_ATTRIBUTE_NO_
#       DEFAULT_DRIVE_LETTER) -- measured in guest-normal.txt on the ESP, the
#       MSR and C: itself. `set id=` only documents changing the type GUID;
#       if it leaves that bit in place, plan::assess's separate "partition
#       attributes set" rule refuses the retyped partition anyway. The lab's
#       own fixture was built by the host's sfdisk, not by Windows, so it
#       ships with attributes=0x0 and would not reproduce this on its own --
#       phase 2 below forces the same bit Set-Partition exposes as
#       -NoDefaultDriveLetter, matching golden's measured value, before
#       retrying the retype.
#
#  Every phase re-surveys with the REAL installer .exe -- its own read of the
#  raw GPT attributes via IOCTL, not PowerShell's boolean properties, is what
#  plan::assess actually acts on -- and prints one greppable REMEDY-<n>: line.
# ─────────────────────────────────────────────────────────────────────────────
$ErrorActionPreference = 'Continue'
$exe = Join-Path $PSScriptRoot 'apex-windows-installer.exe'
if (-not (Test-Path $exe)) { "FATAL: no installer at $exe"; exit 2 }

$LINUX_GUID = '0fc63daf-8483-4772-8e79-3d69d8477de4'
$BASIC_GUID = '{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}'
$dpFile = Join-Path $env:TEMP 'remedy.txt'

function Invoke-Installer {
    param([string]$Cmd)
    & cmd /c "`"$exe`" $Cmd 2>&1"
}

# Pulls the installer's own block for one partition GUID out of its survey
# text and boils it down to one line: the raw attributes it read, the type
# name, what Windows claims about it, and the final verdict. Deliberately not
# re-derived from PowerShell's own view of the partition -- the whole point is
# to check what plan::assess sees, not what this script assumes.
function Get-PartitionReport {
    param([string[]]$Lines, [string]$Guid)
    $text = ($Lines -join "`n")
    $g = [regex]::Escape($Guid.ToLower())
    $pat = "(?ms)^  PARTITION $g\b.*?(?=(\n  PARTITION )|(\nDISK )|(\nCOULD NOT)|(\nsurvey-complete)|\z)"
    $m = [regex]::Match($text, $pat)
    if (-not $m.Success) { return "NOT-FOUND-IN-SURVEY (guid=$Guid)" }
    $block = $m.Value
    $attrs = ([regex]::Match($block, 'attributes (0x[0-9a-f]+)')).Groups[1].Value
    $tname = ([regex]::Match($block, '\d[^\r\n]*?\s{2,}([A-Za-z][A-Za-z ]+?)\s{2,}name')).Groups[1].Value.Trim()
    $claimLines = ($block -split "`n") | Where-Object { $_ -match 'windows-claims:' }
    $claims = ($claimLines -join ' | ').Trim()
    $verdict = 'UNKNOWN'
    if ($block -match 'VERDICT: may be content-checked') { $verdict = 'ALLOWED' }
    elseif ($block -match 'REFUSED \(([^)]+)\)') { $verdict = "REFUSED($($Matches[1]))" }
    "attrs=$attrs type=$tname claims=[$claims] verdict=$verdict"
}

function Run-Diskpart {
    param([string]$Script, [string]$Label)
    Set-Content -Path $dpFile -Value $Script -Encoding ascii
    $out = & diskpart /s $dpFile 2>&1
    "--- diskpart ($Label), exit $LASTEXITCODE ---"
    $out | Out-String -Width 200
}

function Rescan-AndSurvey {
    Update-HostStorageCache
    Start-Sleep -Seconds 5
    Invoke-Installer 'survey'
}

'=== disk lookup ==='
$disk = Get-Disk | Where-Object FriendlyName -eq 'APEX-FIXTURE-A' | Select-Object -First 1
if (-not $disk) { "FATAL: no disk named APEX-FIXTURE-A"; exit 2 }
"fixture-a is disk $($disk.Number)"

$part = Get-Partition -DiskNumber $disk.Number |
        Where-Object { $_.GptType -eq $BASIC_GUID -and $_.Size -gt 15GB }
if (-not $part) { "FATAL: no 'Blank basic' (>15 GB, basic-data) partition found on disk $($disk.Number)"; exit 2 }
$partNum = $part.PartitionNumber
$guid = ($part.Guid -replace '[{}]', '').ToLower()
"blank-basic is partition $partNum, guid $guid, size $($part.Size), letter $($part.DriveLetter)"

'=== REMEDY phase 0: baseline, as the fixture ships ==='
$survey = Invoke-Installer 'survey'
"REMEDY-0-baseline: $(Get-PartitionReport -Lines $survey -Guid $guid)"

'=== REMEDY phase 1: run the EXACT remedy plan.rs prints, against the fixture as-is ==='
Run-Diskpart -Label 'set id (linux)' -Script @"
select disk $($disk.Number)
select partition $partNum
set id=$LINUX_GUID
"@
$survey = Invoke-Installer 'survey'
"REMEDY-1-after-set-id-no-rescan: $(Get-PartitionReport -Lines $survey -Guid $guid)"

'=== REMEDY phase 1b: the same state, after a storage rescan (a stale volume list would read as still-claimed) ==='
$survey = Rescan-AndSurvey
"REMEDY-1b-after-rescan: $(Get-PartitionReport -Lines $survey -Guid $guid)"

'=== REMEDY phase 2: retype back to basic data, force the bit golden.raw carries on every Windows-made partition ==='
$basicGuidPlain = $BASIC_GUID -replace '[{}]', ''
Run-Diskpart -Label 'set id (basic data, revert)' -Script @"
select disk $($disk.Number)
select partition $partNum
set id=$basicGuidPlain
"@
Set-Partition -DiskNumber $disk.Number -PartitionNumber $partNum -NoDefaultDriveLetter $true
$survey = Rescan-AndSurvey
"REMEDY-2-forced-attr: $(Get-PartitionReport -Lines $survey -Guid $guid)"

'=== REMEDY phase 3: retype to Linux again -- does the forced attribute survive the retype? ==='
Run-Diskpart -Label 'set id (linux, with attr forced)' -Script @"
select disk $($disk.Number)
select partition $partNum
set id=$LINUX_GUID
"@
$survey = Rescan-AndSurvey
$phase3 = Get-PartitionReport -Lines $survey -Guid $guid
"REMEDY-3-after-set-id-with-attr: $phase3"

'=== REMEDY phase 4: if attributes still block it, clear them the same deliberate diskpart way ==='
if ($phase3 -match 'REFUSED\(partition attributes set\)') {
    Run-Diskpart -Label 'gpt attributes clear' -Script @"
select disk $($disk.Number)
select partition $partNum
gpt attributes=0x0000000000000000
"@
    $survey = Rescan-AndSurvey
    "REMEDY-4-attrs-cleared: $(Get-PartitionReport -Lines $survey -Guid $guid)"
} else {
    "REMEDY-4-attrs-cleared: SKIPPED (phase 3 was not refused for attributes -- verdict was $phase3)"
}

'=== sanity: the OTHER two partitions on this disk were never touched ==='
$survey = Invoke-Installer 'survey'
$survey | Select-String -Pattern 'PARTITION |Windows data|APEX-TARGET-A|attributes 0x' | Out-String -Width 200

'=== JOB COMPLETE ==='
exit 0
