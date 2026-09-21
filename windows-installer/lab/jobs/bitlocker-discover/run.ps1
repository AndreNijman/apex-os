# -----------------------------------------------------------------------------
#  jobs/bitlocker-discover -- what BitLocker actually looks like on this
#  machine, measured, before a single line of detector is written.
#
#  BitLocker is the one thing in this tool that can cost somebody their Windows
#  install: it seals its key to the boot configuration, so adding a boot entry
#  or a file to the ESP can make the next boot demand a 48-digit recovery key.
#  A refusal built from documentation is a refusal that has never fired. This
#  job produces the three facts the detector needs to be written against:
#
#    1. Is BitLocker installable and usable in this guest at all? On Server it
#       is an optional FEATURE, and installing one needs a reboot -- which is
#       why `winlab run --boots N` exists.
#    2. What does the WMI class report, property by property, for an
#       unencrypted volume, an encrypted one, and a SUSPENDED one? Protection
#       status is the difference between "refuse" and "go ahead", and it has to
#       come from a locale-independent number, not from parsing English.
#    3. What do the first bytes of an encrypted partition look like when read
#       through \\.\PhysicalDriveN -- the one reading that works on a disk
#       Windows does not manage at all?
#    4. **Which PCRs does BitLocker's platform validation profile bind?** TCG
#       assigns PCR 5 to the GPT partition table. Where a profile binds PCR 5,
#       ANY GPT change forces a 48-digit recovery prompt on the next Windows
#       boot -- including merely CREATING APEX's own ESP, not just retyping a
#       partition. `docs/apex-owns-its-esp.md` makes this must-measure item 5
#       and says explicitly that this job "runs `manage-bde -status` and
#       `-protectors -disable` and does not read the profile at all -- adding
#       that is the first thing it needs." This is that addition. The profile
#       is read three ways, because no single one is available on every
#       machine: `manage-bde -protectors -get` (the authoritative per-volume
#       answer, but only for a TPM protector on an encrypted volume), the
#       group-policy registry under HKLM\SOFTWARE\Policies\Microsoft\FVE
#       (which exists whether or not anything is encrypted), and whether the
#       machine has a TPM at all -- because without one there can be no
#       platform validation profile to bind anything.
#
#  Nothing here is an assertion. It is a measurement whose output the detector
#  and its unit tests are then written from.
# -----------------------------------------------------------------------------
$ErrorActionPreference = 'Continue'
$root  = $PSScriptRoot
$phase = 1
$phaseFile = Join-Path $root 'phase.txt'
if (Test-Path $phaseFile) { $phase = [int](Get-Content $phaseFile -Raw).Trim() }

function Emit {
    param([string]$Text)
    $Text
    Add-Content -Path (Join-Path $root "phase-$phase.txt") -Value $Text -Encoding Ascii
}

Emit "=== BITLOCKER DISCOVERY, PHASE $phase ==="
Emit "when: $(Get-Date -Format o)"

# Reads the first $Count bytes at $Offset of a physical drive and returns them
# as hex. Sector-aligned by construction: a raw device handle refuses anything
# else. .Length is never touched -- it reads 0 on a PhysicalDrive handle.
function Get-RawHex {
    param([int]$Disk, [long]$Offset, [int]$Count = 512)
    try {
        $fs = New-Object System.IO.FileStream(
            "\\.\PhysicalDrive$Disk", [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]'ReadWrite')
        $null = $fs.Seek($Offset, [System.IO.SeekOrigin]::Begin)
        $buf = New-Object byte[] $Count
        $got = $fs.Read($buf, 0, $Count)
        $fs.Close()
        $hex = ($buf[0..([Math]::Min($got, $Count) - 1)] | ForEach-Object { '{0:x2}' -f $_ }) -join ''
        $ascii = -join ($buf[0..63] | ForEach-Object {
            if ($_ -ge 32 -and $_ -lt 127) { [char]$_ } else { '.' } })
        return @{ ok = $true; hex = $hex; ascii = $ascii; read = $got }
    } catch {
        return @{ ok = $false; error = "$_" }
    }
}

# Reads the platform validation profile every way it can be read. Called
# before and after encryption, because the two can differ: the policy registry
# is what WOULD be applied, `-protectors -get` is what IS applied to a volume
# that already has a TPM protector.
function Dump-PcrProfile {
    param([string]$Tag, [string[]]$Volumes)
    Emit "--- PLATFORM VALIDATION PROFILE / PCR BINDING ($Tag) ---"

    # (a) Is there a TPM at all? Without one there is no platform validation
    #     profile, and PCR 5 cannot be bound by anything on this machine.
    try {
        $tpm = Get-WmiObject -Namespace 'root\cimv2\security\microsofttpm' `
                             -Class Win32_Tpm -ErrorAction Stop
        if ($tpm) {
            Emit ("tpm-present: YES  SpecVersion=[{0}] ManufacturerIdTxt=[{1}] IsEnabled=[{2}] IsActivated=[{3}] IsOwned=[{4}]" -f `
                  $tpm.SpecVersion, $tpm.ManufacturerIdTxt, $tpm.IsEnabled_InitialValue, `
                  $tpm.IsActivated_InitialValue, $tpm.IsOwned_InitialValue)
        } else {
            Emit "tpm-present: NO (WMI class reachable, zero instances)"
        }
    } catch {
        Emit "tpm-present: NO (Win32_Tpm unavailable -- $($_.Exception.Message))"
    }
    try {
        $gt = Get-Tpm -ErrorAction Stop
        Emit ("get-tpm: TpmPresent=[{0}] TpmReady=[{1}] TpmEnabled=[{2}] TpmActivated=[{3}] TpmOwned=[{4}]" -f `
              $gt.TpmPresent, $gt.TpmReady, $gt.TpmEnabled, $gt.TpmActivated, $gt.TpmOwned)
    } catch { Emit "get-tpm: UNAVAILABLE -- $($_.Exception.Message)" }

    # (b) Group policy. These values exist independently of any encrypted
    #     volume, and are how a fleet turns PCR 5 on. Read every value under
    #     the key rather than probing names -- a name we do not know about is
    #     exactly the thing that would make this measurement wrong.
    foreach ($k in @('HKLM:\SOFTWARE\Policies\Microsoft\FVE',
                     'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\FVE',
                     'HKLM:\SYSTEM\CurrentControlSet\Control\BitLocker')) {
        if (Test-Path $k) {
            Emit "policy-key $k EXISTS:"
            try {
                $props = Get-ItemProperty -Path $k -ErrorAction Stop
                foreach ($n in ($props.PSObject.Properties |
                                Where-Object { $_.Name -notlike 'PS*' })) {
                    Emit ("  {0} = {1}" -f $n.Name, ($n.Value -join ','))
                }
            } catch { Emit "  (could not read: $($_.Exception.Message))" }
        } else {
            Emit "policy-key $k ABSENT"
        }
    }
    if (Test-Path 'HKLM:\SOFTWARE\Policies\Microsoft\FVE') {
        Emit "pcr5-bound-by-policy: see the values above -- a *PlatformValidationProfile* value that lists 5 is what binds it"
    } else {
        Emit "pcr5-bound-by-policy: NO POLICY KEY AT ALL -- nothing on this machine configures the profile, so the OS default applies"
    }

    # (c) The authoritative per-volume answer. `-protectors -get` prints
    #     "PCR Validation Profile:" followed by the bound PCR numbers, but
    #     ONLY for a TPM-backed protector on an encrypted volume.
    foreach ($v in $Volumes) {
        Emit "--- manage-bde -protectors -get $v ---"
        $out = (& cmd /c "manage-bde -protectors -get $v 2>&1") | Out-String -Width 200
        Emit $out
        $lines = $out -split "`r?`n"
        $hdr = -1
        for ($i = 0; $i -lt $lines.Count; $i++) {
            if ($lines[$i] -match '(?i)PCR Validation Profile') { $hdr = $i; break }
        }
        if ($hdr -ge 0) {
            $nums = @()
            # Numbers may sit on the header line itself or on the indented
            # continuation lines under it, several to a line.
            $tail = $lines[$hdr] -replace '(?i).*PCR Validation Profile:?', ''
            foreach ($m in [regex]::Matches($tail, '\d+')) { $nums += $m.Value }
            for ($i = $hdr + 1; $i -lt $lines.Count; $i++) {
                if ($lines[$i] -notmatch '^\s+\S') { break }
                if ($lines[$i] -notmatch '^\s*[\d,\s]+$') { break }
                foreach ($m in [regex]::Matches($lines[$i], '\d+')) { $nums += $m.Value }
            }
            Emit ("pcr-profile[$v]: " + ($nums -join ','))
            Emit ("pcr5-bound[$v]: " + $(if ($nums -contains '5') { 'YES' } else { 'NO' }))
            if ($nums -contains '5') {
                Emit "pcr5-WARNING[$v]: this profile binds PCR 5, the GPT partition table. ANY GPT change -- including merely CREATING APEX's own ESP -- forces a recovery prompt on the next Windows boot on this machine."
            }
        } else {
            Emit "pcr-profile[$v]: NOT PRINTED (no TPM-backed protector on this volume)"
        }
    }
}

function Dump-Wmi {
    param([string]$Tag)
    Emit "--- WMI Win32_EncryptableVolume ($Tag) ---"
    try {
        $inst = Get-WmiObject -Namespace 'root\cimv2\security\MicrosoftVolumeEncryption' `
                              -Class Win32_EncryptableVolume -ErrorAction Stop
        if (-not $inst) { Emit "wmi-instances: 0 (class present, no instances)"; return }
        $n = @($inst).Count
        Emit "wmi-instances: $n"
        foreach ($v in @($inst)) {
            Emit ("wmi-row: DeviceID=[{0}] DriveLetter=[{1}] ProtectionStatus=[{2}] VolumeType=[{3}] PersistentVolumeID=[{4}]" -f `
                  $v.DeviceID, $v.DriveLetter, $v.ProtectionStatus, $v.VolumeType, $v.PersistentVolumeID)
            try { $cs = $v.GetConversionStatus() 
                  Emit ("wmi-conv: DriveLetter=[{0}] ConversionStatus=[{1}] EncryptionPercentage=[{2}] WipingStatus=[{3}] ReturnValue=[{4}]" -f `
                        $v.DriveLetter, $cs.ConversionStatus, $cs.EncryptionPercentage, $cs.WipingPercentage, $cs.ReturnValue)
            } catch { Emit "wmi-conv: FAILED $_" }
            try { $ps = $v.GetProtectionStatus()
                  Emit ("wmi-prot: DriveLetter=[{0}] ProtectionStatus=[{1}] ReturnValue=[{2}]" -f `
                        $v.DriveLetter, $ps.ProtectionStatus, $ps.ReturnValue)
            } catch { Emit "wmi-prot: FAILED $_" }
        }
        Emit "--- one instance, every property ---"
        Emit ((@($inst)[0] | Format-List * | Out-String -Width 200))
    } catch {
        Emit "wmi-class: UNAVAILABLE -- $_"
    }
}

# ---------------------------------------------------------------------------
if ($phase -eq 1) {
    Emit "--- Get-WindowsFeature BitLocker (before) ---"
    Emit ((Get-WindowsFeature -Name BitLocker* | Format-Table Name, InstallState -AutoSize | Out-String -Width 200))
    Emit "--- manage-bde present? ---"
    Emit ("manage-bde: " + (Test-Path "$env:SystemRoot\System32\manage-bde.exe"))
    Dump-Wmi 'before the feature is installed'
    Dump-PcrProfile 'phase 1, before the BitLocker feature is installed' @('C:')

    Emit "--- Install-WindowsFeature BitLocker ---"
    $r = Install-WindowsFeature -Name BitLocker -IncludeAllSubFeature -IncludeManagementTools
    Emit ($r | Format-List Success, RestartNeeded, ExitCode, FeatureResult | Out-String -Width 200)

    Set-Content -Path $phaseFile -Value '2' -Encoding Ascii
    Set-Content -Path (Join-Path $root 'reboot.txt') -Value 'PHASE2' -Encoding Ascii
    Emit "asked the host for another boot"
    exit 0
}

# ---------------------------------------------------------------------------
if ($phase -eq 2) {
    Emit "--- Get-WindowsFeature BitLocker (after reboot) ---"
    Emit ((Get-WindowsFeature -Name BitLocker* | Format-Table Name, InstallState -AutoSize | Out-String -Width 200))

    Emit "--- volumes ---"
    Emit ((Get-Volume | Format-Table DriveLetter, FileSystemLabel, FileSystem, Size -AutoSize | Out-String -Width 200))

    $target = Get-Volume | Where-Object { $_.FileSystemLabel -eq 'WINDATA' -and $_.DriveLetter }
    if (-not $target) { Emit "FATAL: no WINDATA volume with a letter"; exit 3 }
    $L = "$($target.DriveLetter):"
    Emit "target-volume: $L (label WINDATA)"

    # Which physical disk and which byte offset is that volume? Needed for the
    # raw read, and it is the same join the installer has to make.
    $part = Get-Partition | Where-Object { $_.DriveLetter -eq $target.DriveLetter }
    Emit ("target-partition: disk=$($part.DiskNumber) offset=$($part.Offset) size=$($part.Size) guid=$($part.Guid)")

    Dump-Wmi 'unencrypted'
    Dump-PcrProfile 'phase 2, feature installed, nothing encrypted yet' @('C:', $L)
    $raw = Get-RawHex -Disk $part.DiskNumber -Offset $part.Offset -Count 512
    if ($raw.ok) {
        Emit "raw-before-oemid: [$(($raw.hex.Substring(6,16)))]  ascii[0..63]: [$($raw.ascii)]"
        Emit "raw-before-sector0: $($raw.hex)"
    } else { Emit "raw-before: FAILED $($raw.error)" }

    Emit "--- manage-bde -status (unencrypted) ---"
    Emit ((& cmd /c "manage-bde -status $L 2>&1") | Out-String -Width 200)

    Emit "--- Enable-BitLocker $L with a password protector, used space only ---"
    $pw = ConvertTo-SecureString 'ApexLab-Passw0rd-2026!' -AsPlainText -Force
    try {
        $r = Enable-BitLocker -MountPoint $L -PasswordProtector -Password $pw `
                -EncryptionMethod Aes128 -UsedSpaceOnly -SkipHardwareTest -ErrorAction Stop
        Emit ($r | Format-List * | Out-String -Width 200)
    } catch { Emit "Enable-BitLocker FAILED: $_" }

    for ($i = 0; $i -lt 60; $i++) {
        $bv = Get-BitLockerVolume -MountPoint $L -ErrorAction SilentlyContinue
        if ($bv -and $bv.VolumeStatus -eq 'FullyEncrypted') { break }
        Start-Sleep -Seconds 2
    }
    Emit "--- Get-BitLockerVolume after encryption ---"
    Emit ((Get-BitLockerVolume | Format-List MountPoint, VolumeStatus, ProtectionStatus, EncryptionPercentage, KeyProtector | Out-String -Width 200))
    Emit "--- manage-bde -status (encrypted) ---"
    Emit ((& cmd /c "manage-bde -status $L 2>&1") | Out-String -Width 200)

    Dump-Wmi 'encrypted, protection on'
    Dump-PcrProfile 'phase 2, WINDATA encrypted with a PASSWORD protector' @('C:', $L)
    $raw = Get-RawHex -Disk $part.DiskNumber -Offset $part.Offset -Count 512
    if ($raw.ok) {
        Emit "raw-enc-oemid: [$(($raw.hex.Substring(6,16)))]  ascii[0..63]: [$($raw.ascii)]"
        Emit "raw-enc-sector0: $($raw.hex)"
    } else { Emit "raw-enc: FAILED $($raw.error)" }

    Emit "--- SUSPEND, attempt 1: the OS-volume form, on a DATA volume ---"
    Emit "    (kept deliberately: the error it returns is itself the measurement)"
    $suspend1 = (& cmd /c "manage-bde -protectors -disable $L -RebootCount 1 2>&1") | Out-String -Width 200
    Emit $suspend1
    if ($suspend1 -match '0x80310028') {
        Emit "suspend-rebootcount-on-data-volume: REJECTED 0x80310028 (not the operating system drive) -- -RebootCount is an OS-volume-only switch"
    }

    Emit "--- SUSPEND, attempt 2: the data-volume form, no -RebootCount ---"
    Emit ((& cmd /c "manage-bde -protectors -disable $L 2>&1") | Out-String -Width 200)

    # Did it actually take? Ask the number, not the English. ProtectionStatus
    # 1 = on, 0 = off/suspended; it is what the detector will branch on.
    $psNow = $null
    try {
        $inst = Get-WmiObject -Namespace 'root\cimv2\security\MicrosoftVolumeEncryption' `
                              -Class Win32_EncryptableVolume -ErrorAction Stop |
                Where-Object { $_.DriveLetter -eq $L }
        if ($inst) { $psNow = (@($inst)[0]).GetProtectionStatus().ProtectionStatus }
    } catch { Emit "suspend-verify: could not read ProtectionStatus -- $($_.Exception.Message)" }
    Emit "suspend-protectionstatus-after: [$psNow]"
    if ($psNow -eq 0) {
        Emit "SUSPEND-EFFECTIVE: YES -- ProtectionStatus went 1 -> 0 while the volume stays encrypted"
        $tag = 'encrypted, protection SUSPENDED (verified: ProtectionStatus 0)'
    } else {
        Emit "SUSPEND-EFFECTIVE: NO -- ProtectionStatus is still [$psNow]. The dump below is NOT a suspended volume; do not read it as one."
        $tag = 'encrypted, suspend attempted and NOT effective (ProtectionStatus still 1)'
    }

    Emit "--- manage-bde -status (after the suspend attempts) ---"
    Emit ((& cmd /c "manage-bde -status $L 2>&1") | Out-String -Width 200)
    Dump-Wmi $tag
    $raw = Get-RawHex -Disk $part.DiskNumber -Offset $part.Offset -Count 512
    if ($raw.ok) { Emit "raw-susp-sector0: $($raw.hex)" } else { Emit "raw-susp: FAILED $($raw.error)" }

    Emit "--- and the OS volume, which is the one that can lock a user out ---"
    Emit ((& cmd /c "manage-bde -status C: 2>&1") | Out-String -Width 200)
    $cpart = Get-Partition | Where-Object { $_.DriveLetter -eq 'C' }
    if ($cpart) {
        $craw = Get-RawHex -Disk $cpart.DiskNumber -Offset $cpart.Offset -Count 512
        if ($craw.ok) { Emit "raw-C-oemid: [$($craw.hex.Substring(6,16))] ascii: [$($craw.ascii)]" }
    }

    # The last word on must-measure item 5 for THIS guest, stated as a
    # conclusion so nobody has to re-read the transcript to find it -- and
    # stated with its limit attached, because a guest with no TPM cannot
    # answer the question for a machine that has one.
    Emit "--- CONCLUSION: must-measure item 5 (does BitLocker's profile bind PCR 5?) ---"
    Dump-PcrProfile 'final' @('C:', $L)

    Emit '=== JOB COMPLETE ==='
    exit 0
}

Emit "FATAL: unknown phase $phase"
exit 4
