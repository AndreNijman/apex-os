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
    $raw = Get-RawHex -Disk $part.DiskNumber -Offset $part.Offset -Count 512
    if ($raw.ok) {
        Emit "raw-enc-oemid: [$(($raw.hex.Substring(6,16)))]  ascii[0..63]: [$($raw.ascii)]"
        Emit "raw-enc-sector0: $($raw.hex)"
    } else { Emit "raw-enc: FAILED $($raw.error)" }

    Emit "--- SUSPEND: manage-bde -protectors -disable $L -RebootCount 1 ---"
    Emit ((& cmd /c "manage-bde -protectors -disable $L -RebootCount 1 2>&1") | Out-String -Width 200)
    Emit "--- manage-bde -status (suspended) ---"
    Emit ((& cmd /c "manage-bde -status $L 2>&1") | Out-String -Width 200)
    Dump-Wmi 'encrypted, protection suspended'
    $raw = Get-RawHex -Disk $part.DiskNumber -Offset $part.Offset -Count 512
    if ($raw.ok) { Emit "raw-susp-sector0: $($raw.hex)" } else { Emit "raw-susp: FAILED $($raw.error)" }

    Emit "--- and the OS volume, which is the one that can lock a user out ---"
    Emit ((& cmd /c "manage-bde -status C: 2>&1") | Out-String -Width 200)
    $cpart = Get-Partition | Where-Object { $_.DriveLetter -eq 'C' }
    if ($cpart) {
        $craw = Get-RawHex -Disk $cpart.DiskNumber -Offset $cpart.Offset -Count 512
        if ($craw.ok) { Emit "raw-C-oemid: [$($craw.hex.Substring(6,16))] ascii: [$($craw.ascii)]" }
    }

    Emit '=== JOB COMPLETE ==='
    exit 0
}

Emit "FATAL: unknown phase $phase"
exit 4
