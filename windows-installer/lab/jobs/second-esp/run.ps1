# ─────────────────────────────────────────────────────────────────────────────
#  jobs/second-esp -- does Windows tolerate a SECOND EFI System Partition on
#  its own disk, and which one does Windows' own tooling write?
#
#  docs/apex-owns-its-esp.md settled that APEX builds its own ESP and NEVER
#  writes Windows'. It then listed what must be measured before that is claimed
#  to work. migrate-preconditions' card confirms it takes none of the four lab
#  measurements, so they are this unit's. This job takes two of them, and
#  narrows one on purpose:
#
#    #2 "Does Windows tolerate a second ESP? Specifically across a feature
#       update, a repair install and bcdboot."
#         -> the bcdboot half is measured here. THE FEATURE-UPDATE AND
#            REPAIR-INSTALL HALVES ARE NOT DOABLE IN THIS LAB: there is no
#            update media and no repair image. Partial, and labelled partial.
#    #4 "Windows' own boot path byte-identical either side, GPT included,
#       proven by comparison against a pristine fixture."
#         -> as literally worded this CANNOT hold: adding a partition changes
#            the GPT by definition. Read here as "Windows' OWN partition
#            entries and Windows' OWN ESP bytes are unchanged", which is the
#            property that actually protects the user. Verified on the host.
#
#    #1 "does firmware boot the intended one of two ESPs from an explicit NVRAM
#       entry" is NOT attempted. Telling which of two ESPs actually booted
#       needs BootCurrent -- volatile, and absent from the varstore the harness
#       diffs -- or a bootloader payload distinguishable from Windows' own.
#       Attempting it badly would produce a confident wrong answer, which is
#       worse than the gap.
#
#  bcdboot's selection is the interesting question because it is the Windows
#  analogue of the defect migrate-preconditions found on the Linux side: bootc
#  re-discovers the ESP with find_first_colocated_esp() on every upgrade rather
#  than staying on the one it was given. If bcdboot likewise picks by position
#  rather than by "the one I booted from", then a second ESP earlier in
#  partition order is a live hazard on BOTH sides of the machine.
#
#  Everything happens on run.qcow2, a disposable overlay over golden.raw.
# ─────────────────────────────────────────────────────────────────────────────
$ErrorActionPreference = 'Continue'
$root  = $PSScriptRoot
$phase = 1
$phaseFile = Join-Path $root 'phase.txt'
if (Test-Path $phaseFile) { $phase = [int](Get-Content $phaseFile -Raw).Trim() }
$exe = Join-Path $root 'apex-windows-installer.exe'

function Emit { param([string]$T) $T }
function Run-Diskpart {
    param([string[]]$Lines)
    $f = Join-Path $env:TEMP ("dp-{0}.txt" -f (Get-Random))
    Set-Content -Path $f -Value $Lines -Encoding Ascii
    Emit "--- diskpart ---"
    $Lines | ForEach-Object { Emit "    > $_" }
    (& cmd /c "diskpart /s `"$f`" 2>&1") | Out-String -Width 200
    Remove-Item $f -ErrorAction SilentlyContinue
}
# A whole-tree fingerprint: relative path + size + sha256 of every file, so a
# single added or rewritten file shows up as a named difference and not just a
# changed total.
function Get-TreeManifest {
    param([string]$Root)
    if (-not (Test-Path $Root)) { return $null }
    $m = @{}
    Get-ChildItem -LiteralPath $Root -Recurse -File -Force -ErrorAction SilentlyContinue |
        ForEach-Object {
            $rel = $_.FullName.Substring($Root.Length).TrimStart('\')
            try { $h = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
            catch { $h = 'UNREADABLE' }
            $m[$rel] = "$($_.Length):$h"
        }
    return $m
}
# Proves a path is real and writable before any "nothing was written there"
# claim is made about it. A negative result read off an inaccessible path is
# not evidence of anything.
function Test-PathUsable {
    param([string]$Root, [string]$Tag)
    if (-not (Test-Path $Root)) { Emit "PATH-USABLE[$Tag]: NO -- $Root does not exist"; return $false }
    $probe = Join-Path $Root 'apexlab-probe.tmp'
    try {
        Set-Content -Path $probe -Value 'probe' -Encoding Ascii -ErrorAction Stop
        $ok = Test-Path $probe
        Remove-Item $probe -Force -ErrorAction SilentlyContinue
        Emit "PATH-USABLE[$Tag]: $(if ($ok) { 'YES -- created and removed a probe file' } else { 'NO -- write appeared to succeed but the file is not there' })"
        return $ok
    } catch {
        Emit "PATH-USABLE[$Tag]: NO -- $($_.Exception.Message)"
        return $false
    }
}

function Show-Manifest {
    param([string]$Tag, [hashtable]$M)
    Emit "manifest[$Tag]: $($M.Count) files"
    foreach ($k in ($M.Keys | Sort-Object)) { Emit "    $k = $($M[$k])" }
}
function Compare-Manifest {
    param([string]$Tag, $Before, $After)
    if ($null -eq $Before -or $null -eq $After) {
        Emit "delta[$Tag]: NOT MEASURED -- one side of the comparison could not be read"
        Emit "UNCHANGED[$Tag]: UNKNOWN"
        return
    }
    $added   = @($After.Keys  | Where-Object { -not $Before.ContainsKey($_) })
    $removed = @($Before.Keys | Where-Object { -not $After.ContainsKey($_) })
    $changed = @($Before.Keys | Where-Object { $After.ContainsKey($_) -and $After[$_] -ne $Before[$_] })
    Emit "delta[$Tag]: added=$($added.Count) removed=$($removed.Count) changed=$($changed.Count)"
    foreach ($x in $added)   { Emit "    + $x" }
    foreach ($x in $removed) { Emit "    - $x" }
    foreach ($x in $changed) { Emit "    ~ $x" }
    if ($added.Count -eq 0 -and $removed.Count -eq 0 -and $changed.Count -eq 0) {
        Emit "UNCHANGED[$Tag]: YES"
    } else {
        Emit "UNCHANGED[$Tag]: NO"
    }
}
function Show-Layout {
    param([string]$Tag)
    Emit "--- partition layout ($Tag) ---"
    Get-Partition -DiskNumber 0 | Sort-Object PartitionNumber | ForEach-Object {
        $v = Get-Volume -Partition $_ -ErrorAction SilentlyContinue
        Emit ("    #{0} type={1} offset={2} size={3} letter=[{4}] fs=[{5}] label=[{6}]" -f `
              $_.PartitionNumber, $_.GptType, $_.Offset, $_.Size, $_.DriveLetter, $v.FileSystem, $v.FileSystemLabel)
    }
}
function Show-Bcdmgr {
    param([string]$Tag)
    Emit "--- bcdedit /enum {bootmgr} ($Tag) ---"
    $o = (& cmd /c "bcdedit /enum `"{bootmgr}`" 2>&1") | Out-String -Width 200
    Emit $o
    foreach ($l in ($o -split "`r?`n")) {
        if ($l -match '(?i)^\s*device\s+(.+)$') { Emit "bootmgr-device[$Tag]: $($Matches[1].Trim())" }
    }
}

Emit "=== SECOND ESP, PHASE $phase ==="
Emit "when: $(Get-Date -Format o)"

# ─────────────────────────────────────────────────────────────── phase 1 ────
if ($phase -eq 1) {
    Show-Layout 'before'
    Emit "--- what the real installer sees before anything changes ---"
    if (Test-Path $exe) { (& cmd /c "`"$exe`" survey 2>&1") | Out-String -Width 200 }

    # Give Windows' OWN ESP a letter so its contents can be fingerprinted.
    # Reversible, and this is a disposable overlay.
    $espPart = Get-Partition -DiskNumber 0 |
               Where-Object { $_.GptType -eq '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' } |
               Select-Object -First 1
    if (-not $espPart) { Emit 'FATAL: no ESP on disk 0'; exit 2 }
    Emit "windows-esp: partition #$($espPart.PartitionNumber) offset=$($espPart.Offset) size=$($espPart.Size)"
    Run-Diskpart @("select disk 0", "select partition $($espPart.PartitionNumber)", "assign letter=P")

    $espBefore = Get-TreeManifest 'P:\'
    Show-Manifest 'windows-esp BEFORE' $espBefore
    $espBefore | Export-Clixml -Path (Join-Path $root 'esp-before.xml')

    Show-Bcdmgr 'before'

    # Shrink C: and create the second ESP. The shrink is the USER's own act in
    # the real flow -- the tool has no NTFS knowledge and must not grow any --
    # so diskpart is the honest way to perform it here.
    $c = Get-Partition -DiskNumber 0 | Where-Object DriveLetter -eq 'C'
    Emit "C: partition #$($c.PartitionNumber) offset=$($c.Offset) size=$($c.Size) endsAt=$($c.Offset + $c.Size)"
    Run-Diskpart @("select volume C", "shrink desired=600 minimum=520")
    Show-Layout 'after shrink'
    Run-Diskpart @("select disk 0",
                   "create partition efi size=500",
                   "format quick fs=fat32 label=APEXESP",
                   "assign letter=S")
    Show-Layout 'after creating the second ESP'

    $c2 = Get-Partition -DiskNumber 0 | Where-Object DriveLetter -eq 'C'
    Emit "C: after shrink: offset=$($c2.Offset) size=$($c2.Size) endsAt=$($c2.Offset + $c2.Size)"
    Emit "C-OFFSET-UNCHANGED: $(if ($c.Offset -eq $c2.Offset) { 'YES -- the shrink moved only the END' } else { 'NO' })"

    $esp2 = Get-Partition -DiskNumber 0 |
            Where-Object { $_.GptType -eq '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' -and
                           $_.Offset -gt $espPart.Offset } | Select-Object -First 1
    if ($esp2) {
        Emit "second-esp: partition #$($esp2.PartitionNumber) offset=$($esp2.Offset) size=$($esp2.Size) type=$($esp2.GptType)"
        Emit "SECOND-ESP-CREATED: YES"
        Emit "second-esp-is-LATER-in-partition-order-than-windows: $(if ($esp2.Offset -gt $espPart.Offset) { 'YES' } else { 'NO' })"
    } else {
        Emit "SECOND-ESP-CREATED: NO"
    }

    # Did adding a partition disturb Windows' own ESP at all?
    $espMid = Get-TreeManifest 'P:\'
    Compare-Manifest 'windows-esp across shrink+create' $espBefore $espMid

    Emit "--- what the real installer sees with TWO ESPs present ---"
    if (Test-Path $exe) { (& cmd /c "`"$exe`" survey 2>&1") | Out-String -Width 200 }

    # ── THE QUESTION: with two ESPs present and no /s, which one does
    #    Windows' own tool write? ──────────────────────────────────────────
    # Windows does not surface a drive letter for an ESP through
    # Get-Partition.DriveLetter even when diskpart has assigned one, so the
    # access path is PROVEN usable before any claim is based on what is or is
    # not in it.
    $s2usable = Test-PathUsable 'S:\' 'second ESP'
    if (-not $s2usable) {
        Emit "falling back to a directory mount point for the second ESP"
        New-Item -ItemType Directory -Path 'C:\esp2' -Force | Out-Null
        Add-PartitionAccessPath -DiskNumber 0 -PartitionNumber $esp2.PartitionNumber `
            -AccessPath 'C:\esp2' -ErrorAction SilentlyContinue
        $script:S2ROOT = 'C:\esp2'
        $s2usable = Test-PathUsable $script:S2ROOT 'second ESP via mount point'
    } else { $script:S2ROOT = 'S:\' }
    $pUsable = Test-PathUsable 'P:\' "Windows' own ESP"
    $s2Before = Get-TreeManifest $script:S2ROOT
    Emit "second-esp root: $($script:S2ROOT); files before bcdboot: $(if ($null -eq $s2Before) { 'NOT MEASURED' } else { $s2Before.Count })"
    Emit "--- BCDBOOT RUN: bcdboot C:\Windows, NO /s, two ESPs present ---"
    Emit ((& cmd /c "bcdboot C:\Windows 2>&1") | Out-String -Width 200)
    Emit "bcdboot-exit: $LASTEXITCODE"

    $espAfter = Get-TreeManifest 'P:\'
    $s2After  = Get-TreeManifest $script:S2ROOT
    Compare-Manifest 'WINDOWS OWN ESP across bcdboot' $espMid $espAfter
    Compare-Manifest 'SECOND ESP across bcdboot'      $s2Before $s2After
    if ($null -eq $s2Before -or $null -eq $s2After) {
        Emit "BCDBOOT-WROTE-SECOND-ESP: NOT MEASURED -- the second ESP's contents could not be read, so 'nothing was written there' is NOT a result this run may claim"
    } elseif ($s2After.Count -gt 0) {
        Emit "BCDBOOT-WROTE-SECOND-ESP: YES -- $($s2After.Count) files now present where there were $($s2Before.Count)"
    } else {
        Emit "BCDBOOT-WROTE-SECOND-ESP: NO -- the access path was proven usable and it is still empty"
    }
    Show-Bcdmgr 'after bcdboot'
    $espAfter | Export-Clixml -Path (Join-Path $root 'esp-after.xml')

    Show-Layout 'final, phase 1'
    Set-Content -Path $phaseFile -Value '2' -Encoding Ascii
    Set-Content -Path (Join-Path $root 'reboot.txt') -Value 'PHASE2' -Encoding Ascii
    Emit "asked the host for another boot -- the point is that Windows still boots with two ESPs"
    Emit '=== JOB COMPLETE (phase 1) ==='
    exit 0
}

# ─────────────────────────────────────────────────────────────── phase 2 ────
if ($phase -eq 2) {
    Emit "WINDOWS-STILL-BOOTS-WITH-TWO-ESPS: YES -- this phase is running, which is the proof"
    Show-Layout 'after reboot'
    Show-Bcdmgr 'after reboot'
    Emit "--- volumes ---"
    Emit ((Get-Volume | Format-Table DriveLetter, FileSystemLabel, FileSystem, Size -AutoSize | Out-String -Width 200))
    $before = Import-Clixml -Path (Join-Path $root 'esp-after.xml')
    $now    = Get-TreeManifest 'P:\'
    if ($now.Count -eq 0) {
        Emit "note: Windows' ESP lost its drive letter across the reboot; re-assigning"
        Run-Diskpart @("select disk 0", "select partition 1", "assign letter=P")
        $now = Get-TreeManifest 'P:\'
    }
    Compare-Manifest "WINDOWS OWN ESP across the reboot" $before $now
    Emit "--- the real installer, two ESPs, after a reboot ---"
    if (Test-Path $exe) { (& cmd /c "`"$exe`" survey 2>&1") | Out-String -Width 200 }
    Emit '=== JOB COMPLETE ==='
    exit 0
}

Emit "FATAL: unknown phase $phase"
exit 4
