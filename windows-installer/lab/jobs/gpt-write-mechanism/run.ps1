# ─────────────────────────────────────────────────────────────────────────────
#  jobs/gpt-write-mechanism -- the measurement docs/apex-owns-its-esp.md
#  deliberately left undecided.
#
#  The second product decision (2026-09-21, b137f03f) settled that the TOOL
#  changes a partition's type GUID and attributes itself rather than printing
#  diskpart commands for a user to retype. It then said, in those words, that
#  the MECHANISM is not decided because neither candidate is measured:
#
#    - a raw sector read-modify-write of LBA 2-33 through \\.\PhysicalDriveN
#      is narrow in blast radius, "but it is unverified that Windows permits a
#      write to LBA 2-33 on a LIVE SYSTEM DISK at all -- the payload-write
#      proof was to a partition extent, a different protection regime -- and
#      after one, Windows' cached partition view is stale until
#      IOCTL_DISK_UPDATE_PROPERTIES (0x70140)";
#    - GET_DRIVE_LAYOUT_EX -> change one entry -> SET_DRIVE_LAYOUT_EX is wide
#      in API but narrow in intent, "and Windows maintains its own state and
#      the backup GPT for you".
#
#    "Which is safer is one guest boot to find out."
#
#  This is that boot. Nothing here is asserted; every line is a measurement,
#  and the host compares the resulting bytes against a pristine fixture
#  afterwards (hostverify-gpt.py) against the doc's six invariants.
#
#  WHAT IS TOUCHED, AND WHAT IS NOT
#
#    - The LIVE SYSTEM DISK (\\.\PhysicalDrive0, the disk Windows booted from)
#      is probed with a NO-OP write: the exact bytes just read are written back
#      to LBA 2. That answers "is a write to LBA 2-33 on a live system disk
#      permitted at all" -- the doc's stated unknown -- WITHOUT changing a
#      single byte, so a refusal and a success are equally safe outcomes and
#      the guest still boots either way.
#    - The real read-modify-write and the real SET_DRIVE_LAYOUT_EX are done on
#      FIXTURE-A, a non-system disk behind a qcow2 overlay, whose pristine
#      backing file is the host's comparison baseline.
#
#  The target entry is fixture-a's partition 3, "Blank basic": Windows basic
#  data, no recognised filesystem, lettered F:. Retyping it to Linux filesystem
#  and clearing its attributes is exactly the operation the decision
#  authorises, and round 38 already measured that Windows drops the volume
#  object the instant the type GUID changes.
#
#  apex-windows-installer.exe still cannot write. Section 0 of
#  tests/test-windows-installer.sh scans windows-installer/src/ for *.rs only;
#  this job is PowerShell in the lab and is deliberately outside it. The .exe's
#  role here is the same as in payload-write: it is the thing asked whether the
#  result is coherent.
# ─────────────────────────────────────────────────────────────────────────────
$ErrorActionPreference = 'Continue'
$exe = Join-Path $PSScriptRoot 'apex-windows-installer.exe'

$LINUX_GUID  = '0fc63daf-8483-4772-8e79-3d69d8477de4'
$BASIC_GUID  = 'ebd0a0a2-b9e5-4433-87c0-68b6b72699c7'
$SECTOR      = 512
$ENTRIES_LBA = 2
$ENTRY_SIZE  = 128

function Invoke-Installer { param([string]$Cmd) & cmd /c "`"$exe`" $Cmd 2>&1" }

# ── CRC32, the GPT's own checksum (IEEE 802.3, reflected, init/xorout FFFF) ──
#  Written out rather than borrowed: every GPT field that this job rewrites is
#  guarded by one of these, and invariant 2 ("both copies end consistent, with
#  correct CRCs") is unprovable without computing them independently.
# CRC32 lives in C# (see the Add-Type below) and not in PowerShell, because
# PowerShell 5.1 parses `0xFFFFFFFF` as Int32 -1 and `[uint32]0xFFFFFFFF` then
# throws "Value was either too large or too small for a UInt32". The first
# version of this job did exactly that, so every CRC it printed was a failure
# message and every "ok=False" it reported was its own bug rather than a fact
# about the disk. Unsigned 32-bit arithmetic is not something to improvise in
# this shell.
function Get-Crc32 {
    param([byte[]]$Bytes, [int]$Offset = 0, [int]$Length = -1)
    if ($Length -lt 0) { $Length = $Bytes.Length - $Offset }
    return [ApexLab.Native]::Crc32($Bytes, $Offset, $Length)
}

# ── raw device I/O ──────────────────────────────────────────────────────────
#  .Length is never touched on a PhysicalDrive handle: it reads 0 (trap 7,
#  windows-installer-2.md). Sizes come from Get-Disk / the GPT header itself.
function Open-Drive {
    param([string]$Path, [switch]$Write)
    $access = if ($Write) { [System.IO.FileAccess]::ReadWrite } else { [System.IO.FileAccess]::Read }
    $opts   = if ($Write) { [System.IO.FileOptions]::WriteThrough } else { [System.IO.FileOptions]::None }
    # bufferSize 1 means UNBUFFERED. It has to be: with a 64 KiB internal
    # buffer, reading the 512-byte BACKUP GPT header -- which lives in the very
    # LAST sector of the disk -- makes FileStream try to read 64 KiB from that
    # offset, run off the end of the device, and fail the whole read with "The
    # request could not be performed because of an I/O device error". The first
    # version of this job hit exactly that, and because the failure produced a
    # null header rather than an exception at the call site, every later offset
    # computed from it came out 0 and writes aimed at the backup GPT landed on
    # the protective MBR instead.
    New-Object System.IO.FileStream($Path, [System.IO.FileMode]::Open, $access,
        [System.IO.FileShare]::ReadWrite, 1, $opts)
}
function Read-At {
    param([string]$Path, [long]$Offset, [int]$Length)
    $fs = Open-Drive -Path $Path
    try {
        $null = $fs.Seek($Offset, [System.IO.SeekOrigin]::Begin)
        $buf = New-Object byte[] $Length
        $got = 0
        while ($got -lt $Length) {
            $n = $fs.Read($buf, $got, $Length - $got)
            if ($n -le 0) { break }
            $got += $n
        }
        if ($got -ne $Length) { throw "short read at ${Offset}: $got of $Length" }
        return $buf
    } finally { $fs.Dispose() }
}
# Returns $null on success, or the caught exception. A refusal is a structured
# result here, not a script error -- the whole point is to find out which
# writes Windows refuses.
function Write-At {
    param([string]$Path, [long]$Offset, [byte[]]$Bytes)
    try {
        $fs = Open-Drive -Path $Path -Write
        try {
            $null = $fs.Seek($Offset, [System.IO.SeekOrigin]::Begin)
            $fs.Write($Bytes, 0, $Bytes.Length)
            $fs.Flush($true)
        } finally { $fs.Dispose() }
        return $null
    } catch { return $_ }
}
function Hex { param([byte[]]$B, [int]$Off = 0, [int]$Len = 16)
    ($B[$Off..($Off + $Len - 1)] | ForEach-Object { '{0:x2}' -f $_ }) -join '' }
function Sha256Hex { param([byte[]]$B)
    $s = [System.Security.Cryptography.SHA256]::Create()
    try { ($s.ComputeHash($B) | ForEach-Object { '{0:x2}' -f $_ }) -join '' } finally { $s.Dispose() } }

# A GPT type GUID is stored MIXED-ENDIAN: the first three fields little-endian,
# the last two big-endian. .NET's Guid(byte[]) ctor uses exactly that layout,
# so round-tripping through [Guid] is correct and hand-swapping bytes is not.
function Guid-FromBytes { param([byte[]]$B, [int]$Off)
    New-Object Guid (,[byte[]]$B[$Off..($Off + 15)]) }
function Guid-ToBytes  { param([string]$G) (New-Object Guid $G).ToByteArray() }

# A header that could not be read must stop the job. It must NEVER be turned
# into a record of nulls, because `$h.EntriesLba * 512` on a null is 0, and a
# write aimed at the backup GPT then lands on the protective MBR. That is not a
# hypothetical: it is what the first run of this job did to fixture-a's
# overlay.
function Read-GptHeader {
    param([string]$Path, [long]$Lba)
    $h = Read-At -Path $Path -Offset ($Lba * $SECTOR) -Length $SECTOR
    $sig = [System.Text.Encoding]::ASCII.GetString($h, 0, 8)
    if ($sig -ne 'EFI PART') {
        throw "no GPT header at LBA ${Lba} of ${Path}: signature is [$sig], refusing to derive any offset from it"
    }
    [pscustomobject]@{
        Raw        = $h
        Signature  = [System.Text.Encoding]::ASCII.GetString($h, 0, 8)
        HeaderSize = [BitConverter]::ToUInt32($h, 12)
        HeaderCrc  = [BitConverter]::ToUInt32($h, 16)
        MyLba      = [BitConverter]::ToInt64($h, 24)
        AltLba     = [BitConverter]::ToInt64($h, 32)
        EntriesLba = [BitConverter]::ToInt64($h, 72)
        EntryCount = [BitConverter]::ToUInt32($h, 80)
        EntrySize  = [BitConverter]::ToUInt32($h, 84)
        EntriesCrc = [BitConverter]::ToUInt32($h, 88)
    }
}
# The header's own CRC is computed with its CRC field zeroed, over HeaderSize
# bytes -- not over the whole sector. Getting that wrong produces a header that
# looks right to a hex dump and is rejected by every firmware on earth.
function Get-HeaderCrc {
    param([byte[]]$Header, [uint32]$HeaderSize)
    $t = New-Object byte[] $Header.Length
    [Array]::Copy($Header, $t, $Header.Length)
    for ($i = 16; $i -lt 20; $i++) { $t[$i] = 0 }
    Get-Crc32 -Bytes $t -Offset 0 -Length ([int]$HeaderSize)
}
function Test-GptConsistent {
    param([string]$Path, [string]$Tag)
    $ok = $true
    try {
        $pri = Read-GptHeader -Path $Path -Lba 1
        $bak = Read-GptHeader -Path $Path -Lba $pri.AltLba
    } catch {
        "gpt[$Tag]: UNREADABLE -- $($_.Exception.Message)"
        "gpt[$Tag]: BOTH-COPIES-CONSISTENT: False"
        return
    }
    foreach ($pair in @(@{n = 'primary'; h = $pri}, @{n = 'backup'; h = $bak})) {
        $h = $pair.h
        $entBytes = Read-At -Path $Path -Offset ($h.EntriesLba * $SECTOR) `
                            -Length ([int]$h.EntryCount * [int]$h.EntrySize)
        $calcH = Get-HeaderCrc -Header $h.Raw -HeaderSize $h.HeaderSize
        $calcE = Get-Crc32 -Bytes $entBytes
        $sigOk = ($h.Signature -eq 'EFI PART')
        $hOk   = ($calcH -eq $h.HeaderCrc)
        $eOk   = ($calcE -eq $h.EntriesCrc)
        if (-not ($sigOk -and $hOk -and $eOk)) { $ok = $false }
        "gpt[$Tag/$($pair.n)]: sig=[$($h.Signature)] sig-ok=$sigOk header-crc=0x$('{0:x8}' -f $h.HeaderCrc) calc=0x$('{0:x8}' -f $calcH) ok=$hOk entries-crc=0x$('{0:x8}' -f $h.EntriesCrc) calc=0x$('{0:x8}' -f $calcE) ok=$eOk entriesLBA=$($h.EntriesLba)"
    }
    "gpt[$Tag]: BOTH-COPIES-CONSISTENT: $ok"
}

# ── IOCTL bindings ──────────────────────────────────────────────────────────
#  Codes spelled out with their CTL_CODE derivation so a reviewer can check
#  them rather than trust them:
#    GET_DRIVE_LAYOUT_EX  CTL_CODE(7,0x14,BUFFERED,ANY)        = 0x00070050
#    SET_DRIVE_LAYOUT_EX  CTL_CODE(7,0x15,BUFFERED,READ|WRITE) = 0x0007C054
#    UPDATE_PROPERTIES    CTL_CODE(7,0x50,BUFFERED,ANY)        = 0x00070140
Add-Type -Namespace ApexLab -Name Native -MemberDefinition @'
public static uint Crc32(byte[] data, int offset, int length) {
    uint[] table = new uint[256];
    for (uint i = 0; i < 256; i++) {
        uint c = i;
        for (int k = 0; k < 8; k++) c = ((c & 1) != 0) ? (0xEDB88320u ^ (c >> 1)) : (c >> 1);
        table[i] = c;
    }
    uint crc = 0xFFFFFFFFu;
    for (int i = offset; i < offset + length; i++)
        crc = table[(crc ^ data[i]) & 0xFF] ^ (crc >> 8);
    return crc ^ 0xFFFFFFFFu;
}
[DllImport("kernel32.dll", SetLastError = true)]
public static extern bool DeviceIoControl(
    Microsoft.Win32.SafeHandles.SafeFileHandle hDevice,
    uint dwIoControlCode,
    byte[] lpInBuffer, uint nInBufferSize,
    byte[] lpOutBuffer, uint nOutBufferSize,
    out uint lpBytesReturned,
    System.IntPtr lpOverlapped);
'@
$IOCTL_GET_LAYOUT = [uint32]0x00070050
$IOCTL_SET_LAYOUT = [uint32]0x0007C054
$IOCTL_UPDATE     = [uint32]0x00070140

#  DRIVE_LAYOUT_INFORMATION_EX on x64:
#    0   PartitionStyle DWORD
#    4   PartitionCount DWORD
#    8   union { GPT: DiskId GUID(16), StartingUsableOffset(8), UsableLength(8),
#                     MaxPartitionCount DWORD(4) } -> 36, padded to 40
#    48  PARTITION_INFORMATION_EX PartitionEntry[]   (144 bytes each)
#  PARTITION_INFORMATION_EX:
#    +0 PartitionStyle, +8 StartingOffset, +16 PartitionLength,
#    +24 PartitionNumber, +28 RewritePartition, +29 IsServicePartition,
#    +32 union GPT { PartitionType GUID(16), PartitionId GUID(16),
#                    Attributes(8), Name WCHAR[36](72) }
$LAYOUT_ENTRIES_OFF = 48
$ENTRY_STRIDE       = 144
$ENTRY_REWRITE_OFF  = 28
$ENTRY_TYPE_OFF     = 32
$ENTRY_ID_OFF       = 48
$ENTRY_ATTR_OFF     = 64

function Invoke-Ioctl {
    param([string]$Path, [uint32]$Code, [byte[]]$In, [int]$OutLen, [string]$What)
    $fs = Open-Drive -Path $Path -Write
    try {
        $out = if ($OutLen -gt 0) { New-Object byte[] $OutLen } else { $null }
        $br  = [uint32]0
        $inLen = if ($null -eq $In) { [uint32]0 } else { [uint32]$In.Length }
        $ok = [ApexLab.Native]::DeviceIoControl($fs.SafeFileHandle, $Code, $In, $inLen,
                                                $out, [uint32]$OutLen, [ref]$br, [System.IntPtr]::Zero)
        $err = if ($ok) { 0 } else { [System.Runtime.InteropServices.Marshal]::GetLastWin32Error() }
        return [pscustomobject]@{ Ok = $ok; Error = $err; Bytes = $br; Out = $out; What = $What }
    } finally { $fs.Dispose() }
}

# ── which disks are we talking about ────────────────────────────────────────
'=== disks ==='
$sysDisk = Get-Disk | Where-Object { $_.IsSystem -or $_.IsBoot } | Select-Object -First 1
if (-not $sysDisk) { $sysDisk = Get-Disk | Where-Object FriendlyName -eq 'APEX-LAB-SYSTEM' | Select-Object -First 1 }
$faDisk  = Get-Disk | Where-Object FriendlyName -eq 'APEX-FIXTURE-A' | Select-Object -First 1
if (-not $sysDisk) { 'FATAL: no system disk found'; exit 2 }
if (-not $faDisk)  { 'FATAL: no APEX-FIXTURE-A found'; exit 2 }
$sysPath = "\\.\PhysicalDrive$($sysDisk.Number)"
$faPath  = "\\.\PhysicalDrive$($faDisk.Number)"
"system disk : $($sysDisk.FriendlyName) number=$($sysDisk.Number) IsSystem=$($sysDisk.IsSystem) IsBoot=$($sysDisk.IsBoot) -> $sysPath"
"fixture-a   : $($faDisk.FriendlyName) number=$($faDisk.Number) IsSystem=$($faDisk.IsSystem) IsBoot=$($faDisk.IsBoot) -> $faPath"

''
'################ PHASE A -- may Windows be asked to write LBA 2-33 of the LIVE SYSTEM DISK? ################'
'  The doc calls this unverified. It is probed with a NO-OP: the exact bytes'
'  just read are written straight back, so a success changes nothing and a'
'  refusal costs nothing. Either answer is safe and the guest still boots.'
Test-GptConsistent -Path $sysPath -Tag 'sys/before'
$sysEnt0 = Read-At -Path $sysPath -Offset ($ENTRIES_LBA * $SECTOR) -Length $SECTOR
"sys-lba2-sha256-before: $(Sha256Hex $sysEnt0)"
$e = Write-At -Path $sysPath -Offset ($ENTRIES_LBA * $SECTOR) -Bytes $sysEnt0
if ($e) {
    "SYS-RAW-GPT-WRITE: REFUSED -- $($e.Exception.GetType().Name): $($e.Exception.Message) [HResult=$($e.Exception.HResult)]"
} else {
    'SYS-RAW-GPT-WRITE: PERMITTED -- Windows allowed a write to LBA 2 of the disk it booted from'
}
$sysEnt1 = Read-At -Path $sysPath -Offset ($ENTRIES_LBA * $SECTOR) -Length $SECTOR
"sys-lba2-sha256-after:  $(Sha256Hex $sysEnt1)"
"SYS-LBA2-UNCHANGED: $(if ((Sha256Hex $sysEnt0) -eq (Sha256Hex $sysEnt1)) { 'YES' } else { 'NO -- THE NO-OP CHANGED BYTES, WHICH MUST NOT HAPPEN' })"

'--- and the same question through the layout IOCTL, still on the live system disk ---'
$g = Invoke-Ioctl -Path $sysPath -Code $IOCTL_GET_LAYOUT -In $null -OutLen 16384 -What 'GET_DRIVE_LAYOUT_EX(sys)'
"SYS-GET_DRIVE_LAYOUT_EX: ok=$($g.Ok) err=$($g.Error) bytes=$($g.Bytes)"
if ($g.Ok) {
    $style = [BitConverter]::ToUInt32($g.Out, 0); $count = [BitConverter]::ToUInt32($g.Out, 4)
    "  PartitionStyle=$style (1=GPT) PartitionCount=$count"
    $s = Invoke-Ioctl -Path $sysPath -Code $IOCTL_SET_LAYOUT -In $g.Out[0..([int]$g.Bytes - 1)] -OutLen 0 -What 'SET_DRIVE_LAYOUT_EX(sys, unmodified)'
    "SYS-SET_DRIVE_LAYOUT_EX (handing back the UNMODIFIED layout): ok=$($s.Ok) err=$($s.Error)"
}
Test-GptConsistent -Path $sysPath -Tag 'sys/after'

''
'################ the target entry on fixture-a ################'
$p3 = Get-Partition -DiskNumber $faDisk.Number |
      Where-Object { $_.GptType -eq "{$BASIC_GUID}" -and $_.Size -gt 15GB } | Select-Object -First 1
if (-not $p3) { 'FATAL: no Blank-basic partition on fixture-a'; exit 2 }
$p3id = ($p3.Guid -replace '[{}]', '').ToLower()
"target: offset=$($p3.Offset) size=$($p3.Size) letter=$($p3.DriveLetter) type=$($p3.GptType) id=$p3id"

try {
    $pri = Read-GptHeader -Path $faPath -Lba 1
    $bak = Read-GptHeader -Path $faPath -Lba $pri.AltLba
} catch {
    "FATAL: could not read both GPT copies of fixture-a -- $($_.Exception.Message)"
    exit 2
}
if ($null -eq $pri -or $null -eq $bak -or $pri.MyLba -ne 1 -or $bak.MyLba -ne $pri.AltLba) {
    "FATAL: GPT headers did not read coherently (pri.MyLba=$($pri.MyLba) pri.AltLba=$($pri.AltLba) bak.MyLba=$($bak.MyLba)). Refusing to write anything."
    exit 2
}
"gpt-geometry: primary header LBA $($pri.MyLba), entries LBA $($pri.EntriesLba); backup header LBA $($bak.MyLba), entries LBA $($bak.EntriesLba); $($pri.EntryCount) x $($pri.EntrySize) bytes"
$entLen = [int]$pri.EntryCount * [int]$pri.EntrySize
$priEnt = Read-At -Path $faPath -Offset ($pri.EntriesLba * $SECTOR) -Length $entLen
$bakEnt = Read-At -Path $faPath -Offset ($bak.EntriesLba * $SECTOR) -Length $entLen

# Invariant 6: the entry is located by a FRESH read of the table on disk, and
# identified by its unique partition GUID -- never by an index a caller passed
# in and never from a cached layout.
$idx = -1
for ($i = 0; $i -lt [int]$pri.EntryCount; $i++) {
    $o = $i * [int]$pri.EntrySize
    if ((Guid-FromBytes -B $priEnt -Off ($o + 16)).ToString() -eq $p3id) { $idx = $i; break }
}
if ($idx -lt 0) { 'FATAL: target entry not found in a fresh read of the on-disk GPT'; exit 2 }
$eoff = $idx * [int]$pri.EntrySize
"entry-index: $idx  (located by unique partition GUID in a fresh on-disk read)"
"entry-type-before: $((Guid-FromBytes -B $priEnt -Off $eoff).ToString())"
"entry-attrs-before: 0x$('{0:x16}' -f [BitConverter]::ToUInt64($priEnt, $eoff + 48))"

# Invariant 3: a backup of BOTH copies is written to a file before anything
# changes. Written to the transport volume so the host gets it too.
$backupPath = Join-Path $PSScriptRoot 'gpt-backup-fixture-a.bin'
$bundle = New-Object byte[] ($SECTOR * 2 + $entLen * 2)
[Array]::Copy($pri.Raw, 0, $bundle, 0, $SECTOR)
[Array]::Copy($priEnt, 0, $bundle, $SECTOR, $entLen)
[Array]::Copy($bak.Raw, 0, $bundle, $SECTOR + $entLen, $SECTOR)
[Array]::Copy($bakEnt, 0, $bundle, $SECTOR * 2 + $entLen, $entLen)
[System.IO.File]::WriteAllBytes($backupPath, $bundle)
"gpt-backup-written: $backupPath ($($bundle.Length) bytes) sha256=$(Sha256Hex $bundle)"
$gptRegionBefore = Sha256Hex (Read-At -Path $faPath -Offset 0 -Length ($SECTOR * 34))
"fa-primary-gpt-region-sha256-before: $gptRegionBefore"

# Rewrites one entry in an entries array and returns a NEW array. Mutating in
# place would make the "restore the saved bytes" undo test meaningless.
function Set-EntryType {
    param([byte[]]$Entries, [int]$EntryOff, [string]$TypeGuid, [uint64]$Attributes)
    $n = New-Object byte[] $Entries.Length
    [Array]::Copy($Entries, $n, $Entries.Length)
    [Array]::Copy((Guid-ToBytes $TypeGuid), 0, $n, $EntryOff, 16)
    [Array]::Copy([BitConverter]::GetBytes($Attributes), 0, $n, $EntryOff + 48, 8)
    return $n
}
# Writes an entries array and the header that checksums it, to ONE copy.
function Write-GptCopy {
    param([string]$Path, $Header, [byte[]]$Entries)
    $crc = Get-Crc32 -Bytes $Entries
    $h = New-Object byte[] $Header.Raw.Length
    [Array]::Copy($Header.Raw, $h, $h.Length)
    [Array]::Copy([BitConverter]::GetBytes([uint32]$crc), 0, $h, 88, 4)
    $hc = Get-HeaderCrc -Header $h -HeaderSize $Header.HeaderSize
    [Array]::Copy([BitConverter]::GetBytes([uint32]$hc), 0, $h, 16, 4)
    $e1 = Write-At -Path $Path -Offset ($Header.EntriesLba * $SECTOR) -Bytes $Entries
    if ($e1) { return "entries write FAILED: $($e1.Exception.Message)" }
    $e2 = Write-At -Path $Path -Offset ($Header.MyLba * $SECTOR) -Bytes $h
    if ($e2) { return "header write FAILED: $($e2.Exception.Message)" }
    return $null
}
function Show-WindowsView {
    param([string]$Tag)
    $q = Get-Partition -DiskNumber $faDisk.Number -ErrorAction SilentlyContinue |
         Where-Object { ($_.Guid -replace '[{}]', '').ToLower() -eq $p3id }
    if ($q) {
        $vol = Get-Volume -Partition $q -ErrorAction SilentlyContinue
        "windows-view[$Tag]: GptType=$($q.GptType) letter=[$($q.DriveLetter)] fs=[$($vol.FileSystem)]"
    } else {
        "windows-view[$Tag]: no partition object with that GUID (the volume/partition is gone from Windows' view)"
    }
}

''
'################ MECHANISM 1 -- raw sector read-modify-write of LBA 2-33 ################'
$newPri = Set-EntryType -Entries $priEnt -EntryOff $eoff -TypeGuid $LINUX_GUID -Attributes 0
$newBak = Set-EntryType -Entries $bakEnt -EntryOff $eoff -TypeGuid $LINUX_GUID -Attributes 0
$r1 = Write-GptCopy -Path $faPath -Header $pri -Entries $newPri
"M1-primary-write: $(if ($r1) { $r1 } else { 'OK' })"
$r2 = Write-GptCopy -Path $faPath -Header $bak -Entries $newBak
"M1-backup-write:  $(if ($r2) { $r2 } else { 'OK (written BY US -- the raw mechanism must maintain the second copy itself)' })"
Test-GptConsistent -Path $faPath -Tag 'fa/after-M1'
Show-WindowsView 'M1, before UPDATE_PROPERTIES'
$u = Invoke-Ioctl -Path $faPath -Code $IOCTL_UPDATE -In $null -OutLen 0 -What 'UPDATE_PROPERTIES'
"M1-IOCTL_DISK_UPDATE_PROPERTIES: ok=$($u.Ok) err=$($u.Error)"
Show-WindowsView 'M1, after UPDATE_PROPERTIES'
if (Test-Path $exe) {
    '--- the real installer, on the M1 result ---'
    Invoke-Installer "survey" | Out-String -Width 200 |
        Select-String -Pattern 'PARTITION|VERDICT|REFUSED|windows-claims|attributes' | Out-String -Width 200
}

''
'################ UNDO -- restore the file written before the change ################'
'  Invariant 3. Restores the TABLE, not partition contents.'
$sav = [System.IO.File]::ReadAllBytes($backupPath)
$savPriH = New-Object byte[] $SECTOR; [Array]::Copy($sav, 0, $savPriH, 0, $SECTOR)
$savPriE = New-Object byte[] $entLen; [Array]::Copy($sav, $SECTOR, $savPriE, 0, $entLen)
$savBakH = New-Object byte[] $SECTOR; [Array]::Copy($sav, $SECTOR + $entLen, $savBakH, 0, $SECTOR)
$savBakE = New-Object byte[] $entLen; [Array]::Copy($sav, $SECTOR * 2 + $entLen, $savBakE, 0, $entLen)
foreach ($w in @(
    @{n = 'primary entries'; off = $pri.EntriesLba * $SECTOR; b = $savPriE},
    @{n = 'primary header';  off = $pri.MyLba     * $SECTOR; b = $savPriH},
    @{n = 'backup entries';  off = $bak.EntriesLba * $SECTOR; b = $savBakE},
    @{n = 'backup header';   off = $bak.MyLba     * $SECTOR; b = $savBakH})) {
    $e = Write-At -Path $faPath -Offset $w.off -Bytes $w.b
    "undo-write[$($w.n)] at offset $($w.off): $(if ($e) { "FAILED -- $($e.Exception.Message)" } else { 'OK' })"
}
$null = Invoke-Ioctl -Path $faPath -Code $IOCTL_UPDATE -In $null -OutLen 0 -What 'UPDATE_PROPERTIES'
$gptRegionAfterUndo = Sha256Hex (Read-At -Path $faPath -Offset 0 -Length ($SECTOR * 34))
"fa-primary-gpt-region-sha256-after-undo: $gptRegionAfterUndo"
"UNDO-BYTE-EXACT: $(if ($gptRegionAfterUndo -eq $gptRegionBefore) { 'YES -- the primary GPT region is byte-identical to the pre-change backup' } else { 'NO' })"
Test-GptConsistent -Path $faPath -Tag 'fa/after-undo'
Show-WindowsView 'after undo'

''
'################ MECHANISM 2 -- GET_DRIVE_LAYOUT_EX -> patch one entry -> SET_DRIVE_LAYOUT_EX ################'
$g2 = Invoke-Ioctl -Path $faPath -Code $IOCTL_GET_LAYOUT -In $null -OutLen 16384 -What 'GET_DRIVE_LAYOUT_EX(fa)'
"M2-GET_DRIVE_LAYOUT_EX: ok=$($g2.Ok) err=$($g2.Error) bytes=$($g2.Bytes)"
if (-not $g2.Ok) {
    'M2: cannot proceed without a layout'
} else {
    $lay = New-Object byte[] ([int]$g2.Bytes)
    [Array]::Copy($g2.Out, $lay, [int]$g2.Bytes)
    $style = [BitConverter]::ToUInt32($lay, 0)
    $count = [BitConverter]::ToUInt32($lay, 4)
    "  PartitionStyle=$style (1=GPT) PartitionCount=$count  (invariant 6: this is a FRESH read, nothing reconstructed)"

    # Locate the same entry again, in the kernel's own layout, by unique GUID.
    $li = -1
    for ($i = 0; $i -lt [int]$count; $i++) {
        $b = $LAYOUT_ENTRIES_OFF + $i * $ENTRY_STRIDE
        if ($b + $ENTRY_STRIDE -gt $lay.Length) { break }
        if ((Guid-FromBytes -B $lay -Off ($b + $ENTRY_ID_OFF)).ToString() -eq $p3id) { $li = $i; break }
    }
    "  layout-entry-index: $li"
    if ($li -lt 0) {
        'M2: target entry not present in the kernel layout'
    } else {
        $b = $LAYOUT_ENTRIES_OFF + $li * $ENTRY_STRIDE
        "  layout-type-before:  $((Guid-FromBytes -B $lay -Off ($b + $ENTRY_TYPE_OFF)).ToString())"
        "  layout-attrs-before: 0x$('{0:x16}' -f [BitConverter]::ToUInt64($lay, $b + $ENTRY_ATTR_OFF))"

        # Invariant 1: exactly one entry's type GUID and attributes. Every
        # other byte of the layout is handed back exactly as the kernel gave
        # it. RewritePartition is set ONLY on this entry.
        [Array]::Copy((Guid-ToBytes $LINUX_GUID), 0, $lay, $b + $ENTRY_TYPE_OFF, 16)
        [Array]::Copy([BitConverter]::GetBytes([uint64]0), 0, $lay, $b + $ENTRY_ATTR_OFF, 8)
        $lay[$b + $ENTRY_REWRITE_OFF] = 1

        $s2 = Invoke-Ioctl -Path $faPath -Code $IOCTL_SET_LAYOUT -In $lay -OutLen 0 -What 'SET_DRIVE_LAYOUT_EX(fa)'
        "M2-SET_DRIVE_LAYOUT_EX: ok=$($s2.Ok) err=$($s2.Error)"

        # Does Windows refresh its own view without being told to? That is the
        # whole claimed advantage of this mechanism over the raw one.
        Show-WindowsView 'M2, immediately after SET, no UPDATE_PROPERTIES'
        $u2 = Invoke-Ioctl -Path $faPath -Code $IOCTL_UPDATE -In $null -OutLen 0 -What 'UPDATE_PROPERTIES'
        "M2-IOCTL_DISK_UPDATE_PROPERTIES: ok=$($u2.Ok) err=$($u2.Error)"
        Show-WindowsView 'M2, after UPDATE_PROPERTIES'

        # Did Windows maintain the SECOND copy and the CRCs for us? The raw
        # mechanism had to do both by hand; the doc's claim for this one is
        # that the kernel does it.
        '--- did the kernel write both GPT copies, with correct CRCs, by itself? ---'
        Test-GptConsistent -Path $faPath -Tag 'fa/after-M2'
        $priAfter = Read-GptHeader -Path $faPath -Lba 1
        $entAfter = Read-At -Path $faPath -Offset ($priAfter.EntriesLba * $SECTOR) -Length $entLen
        "M2-entry-type-on-disk:  $((Guid-FromBytes -B $entAfter -Off $eoff).ToString())"
        "M2-entry-attrs-on-disk: 0x$('{0:x16}' -f [BitConverter]::ToUInt64($entAfter, $eoff + 48))"

        # Invariant 1, checked in-guest as well as on the host: how many bytes
        # of the 16 KiB entries array differ from the pre-change backup, and
        # are they all inside this one 128-byte entry?
        $diff = @()
        for ($i = 0; $i -lt $entLen; $i++) { if ($entAfter[$i] -ne $savPriE[$i]) { $diff += $i } }
        $inside = $true
        foreach ($d in $diff) { if ($d -lt $eoff -or $d -ge $eoff + [int]$pri.EntrySize) { $inside = $false } }
        "M2-entries-bytes-changed: $($diff.Count)  all-inside-target-entry: $inside"
        if ($diff.Count -gt 0) {
            "M2-changed-byte-offsets-within-entry: $(($diff | ForEach-Object { $_ - $eoff }) -join ',')"
        }
    }
}

''
'################ final state, as the real installer sees it ################'
if (Test-Path $exe) { Invoke-Installer 'survey' | Out-String -Width 200 } else { 'no installer on the transport' }

$gptFinal = Sha256Hex (Read-At -Path $faPath -Offset 0 -Length ($SECTOR * 34))
"fa-primary-gpt-region-sha256-final: $gptFinal"
"fa-gpt-region-changed-vs-before: $(if ($gptFinal -ne $gptRegionBefore) { 'YES (mechanism 2 left its change in place for the host to verify)' } else { 'NO' })"

'=== JOB COMPLETE ==='
exit 0
