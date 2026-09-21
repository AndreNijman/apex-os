# ─────────────────────────────────────────────────────────────────────────────
#  jobs/payload-write — priority 4, payload deployment, SYNTHETIC payload only.
#
#  Nothing in apex-windows-installer.exe writes to a disk: tests/test-
#  windows-installer.sh section 0 denylists WriteFile/GENERIC_WRITE/etc. by
#  name across windows-installer/src/ and allowlists every declared IOCTL/
#  FSCTL constant. That gate is deliberate, not a gap to work around, so this
#  measures the PLATFORM claim ARCHITECTURE.md makes -- from PowerShell, which
#  is allowed to write -- rather than adding a write path to the binary.
#
#  ARCHITECTURE.md's "Exclusivity" section is the spec under test:
#
#    - Windows creates NO VOLUME OBJECT for a Linux-filesystem-type partition,
#      so there is nothing to FSCTL_LOCK_VOLUME. Safety is offset validation
#      (every offset/length checked against the partition extent before the
#      call) plus a volume re-enumeration immediately before each write.
#    - "Windows itself is the backstop: it refuses writes through a
#      PhysicalDrive handle to regions a mounted volume owns." That line had
#      never been checked against a real Windows guest before this job.
#
#  Three writes are attempted through \\.\PhysicalDriveN at verified offsets:
#
#    1. into the eligible Linux-filesystem partition (APEX-TARGET-A) -- MUST
#       succeed, and MUST land exactly at the verified offset and nowhere
#       else. Verified twice: read back in this same guest session, and
#       independently from the HOST afterwards by reading the qcow2 overlay
#       directly (a fresh reader entirely outside Windows).
#    2. into the mounted NTFS partition ("Windows data") -- expected to be
#       refused BY WINDOWS, not by any check this script performs.
#    3. into the lettered but RAW (no recognised filesystem) "Blank basic"
#       partition -- also expected to be refused, and worth checking
#       separately from (2): Windows' documented direct-write protection is
#       usually described in terms of NTFS; whether it also covers a mounted
#       volume with no recognised filesystem at all is exactly the kind of
#       platform detail ARCHITECTURE.md's ownership rule cannot afford to be
#       wrong about, since assess() relies on the SAME claim to refuse (3)
#       today.
#
#  `File::metadata().len()` reads 0 on a `\\.\PhysicalDriveN` handle (trap 7,
#  windows-installer-2.md) -- this script never touches `.Length` on the
#  FileStream for the same reason; sizes come from Get-Partition instead.
# ─────────────────────────────────────────────────────────────────────────────
$ErrorActionPreference = 'Continue'
$exe = Join-Path $PSScriptRoot 'apex-windows-installer.exe'
if (-not (Test-Path $exe)) { "FATAL: no installer at $exe"; exit 2 }

$LINUX_GUID = '{0fc63daf-8483-4772-8e79-3d69d8477de4}'
$BASIC_GUID = '{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}'
$PAYLOAD_LEN = 4MB
$SAMPLE_LEN = 512

function Invoke-Installer {
    param([string]$Cmd)
    & cmd /c "`"$exe`" $Cmd 2>&1"
}

#  A $null argument to ComputeHash's overloaded (byte[] | Stream) signature
#  is genuinely ambiguous to the .NET method binder and PowerShell reports it
#  as "Multiple ambiguous overloads found ... argument count: 1" -- a message
#  that names the wrong problem. Guard explicitly so a failed upstream read
#  reads as NULL-INPUT here, not as a confusing overload error three
#  functions away from the thing that actually failed.
function Get-Sha256Hex {
    param([byte[]]$Bytes)
    if ($null -eq $Bytes) { return 'NULL-INPUT' }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try { ($sha.ComputeHash($Bytes, 0, $Bytes.Length) | ForEach-Object { $_.ToString('x2') }) -join '' }
    finally { $sha.Dispose() }
}

# Read Length bytes at Offset from the physical drive, for verification only
# -- never used to decide whether a write is safe, only to check one after
# the fact. Returns $null on failure rather than letting an exception
# propagate -- every call site can then treat "could not read" the same way
# Write-DrRegion's callers already treat "could not write": a reported
# outcome, not a script crash.
#
# The short-read branch uses Write-Warning, not a bare string: any
# uncaptured expression inside a PowerShell function is added to its
# pipeline OUTPUT, so a bare string here would have turned this function's
# return value into a two-element array (the warning text, then the byte[])
# on exactly the path a real short read would take -- silently corrupting
# every caller expecting a byte[].
function Read-DrRegion {
    param([string]$Path, [long]$Offset, [int]$Length)
    try {
        $fs = New-Object System.IO.FileStream($Path, [System.IO.FileMode]::Open, `
            [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
        try {
            $fs.Seek($Offset, [System.IO.SeekOrigin]::Begin) | Out-Null
            $buf = New-Object byte[] $Length
            $got = $fs.Read($buf, 0, $Length)
            if ($got -ne $Length) { Write-Warning "short read at offset ${Offset}: got $got of $Length" }
            return $buf
        } finally { $fs.Dispose() }
    } catch {
        Write-Warning "Read-DrRegion($Path, $Offset, $Length) failed: $($_.Exception.Message)"
        return $null
    }
}

# Write Bytes at Offset to the physical drive. Returns $null on success, or
# the caught exception on failure -- a refusal is an expected, structured
# result here, not a script error.
function Write-DrRegion {
    param([string]$Path, [long]$Offset, [byte[]]$Bytes)
    try {
        $fs = New-Object System.IO.FileStream($Path, [System.IO.FileMode]::Open, `
            [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::ReadWrite, `
            65536, [System.IO.FileOptions]::WriteThrough)
        try {
            $fs.Seek($Offset, [System.IO.SeekOrigin]::Begin) | Out-Null
            $fs.Write($Bytes, 0, $Bytes.Length)
            $fs.Flush($true)
        } finally { $fs.Dispose() }
        return $null
    } catch {
        return $_
    }
}

'=== disk lookup ==='
$disk = Get-Disk | Where-Object FriendlyName -eq 'APEX-FIXTURE-A' | Select-Object -First 1
if (-not $disk) { "FATAL: no disk named APEX-FIXTURE-A"; exit 2 }
$drPath = "\\.\PhysicalDrive$($disk.Number)"
"fixture-a is disk $($disk.Number), reached as $drPath"

$p1 = Get-Partition -DiskNumber $disk.Number | Where-Object { $_.GptType -eq $LINUX_GUID -and $_.Size -gt 15GB }
$p2 = Get-Partition -DiskNumber $disk.Number | Where-Object { $_.GptType -eq $BASIC_GUID -and $_.Size -lt 2GB }
$p3 = Get-Partition -DiskNumber $disk.Number | Where-Object { $_.GptType -eq $BASIC_GUID -and $_.Size -gt 15GB }
if (-not $p1) { "FATAL: no eligible (Linux filesystem, >15GB) partition found"; exit 2 }
if (-not $p2) { "FATAL: no NTFS-sized Windows-data partition found"; exit 2 }
if (-not $p3) { "FATAL: no Blank-basic partition found"; exit 2 }
"p1 (APEX-TARGET-A, eligible)  offset=$($p1.Offset) size=$($p1.Size)"
"p2 (Windows data, NTFS)       offset=$($p2.Offset) size=$($p2.Size) letter=$($p2.DriveLetter)"
"p3 (Blank basic, RAW+lettered) offset=$($p3.Offset) size=$($p3.Size) letter=$($p3.DriveLetter)"

# ── the synthetic payload ────────────────────────────────────────────────────
$rng = New-Object System.Random(12345)  # fixed seed: reproducible, not secret
$payload = New-Object byte[] $PAYLOAD_LEN
$rng.NextBytes($payload)
$payloadHash = Get-Sha256Hex -Bytes $payload
"PAYLOAD-SHA256: $payloadHash"
"PAYLOAD-LEN: $($payload.Length)"

# ── Exclusivity, re-asked immediately before the write, exactly as ──────────
#    ARCHITECTURE.md says the real code must: a volume list taken ten seconds
#    ago is not evidence about now. `inspect` already performs this re-
#    enumeration and content-scans the whole partition; running it here is
#    the closest thing to "the real code's blessing" that exists today, since
#    no write path exists yet to call it FROM.
$p1Guid = ($p1.Guid -replace '[{}]', '').ToLower()
'=== exclusivity re-check via the real installer, immediately before writing ==='
$inspectOut = Invoke-Installer "inspect $p1Guid"
$inspectOut | Out-String -Width 200
$inspectExit = $LASTEXITCODE
"inspect-exit: $inspectExit"
if ($inspectExit -ne 0) {
    "FATAL: the real installer refused this partition; a payload write would never be reached in production. Stopping before writing anything."
    exit 3
}

# ── write 1: the eligible partition -- MUST succeed ──────────────────────────
'=== write 1: eligible Linux-filesystem partition (expect: success) ==='
$err1 = Write-DrRegion -Path $drPath -Offset $p1.Offset -Bytes $payload
if ($err1) {
    "WRITE-1-RESULT: FAILED (unexpected) -- $($err1.Exception.GetType().FullName): $($err1.Exception.Message)"
} else {
    "WRITE-1-RESULT: SUCCEEDED"
    $readback = Read-DrRegion -Path $drPath -Offset $p1.Offset -Length $payload.Length
    $readbackHash = Get-Sha256Hex -Bytes $readback
    "WRITE-1-READBACK-SHA256: $readbackHash"
    if ($readbackHash -eq $payloadHash) { "WRITE-1-VERIFY: MATCH" } else { "WRITE-1-VERIFY: MISMATCH" }
}

# ── write 2: the mounted NTFS partition -- MUST be refused by Windows ───────
$sample = New-Object byte[] $SAMPLE_LEN
(New-Object System.Random(999)).NextBytes($sample)
'=== write 2: mounted NTFS partition, "Windows data" (expect: refused BY WINDOWS) ==='
$before2 = Read-DrRegion -Path $drPath -Offset $p2.Offset -Length $SAMPLE_LEN
"p2-before-sha256: $(Get-Sha256Hex -Bytes $before2)"
$err2 = Write-DrRegion -Path $drPath -Offset $p2.Offset -Bytes $sample
if ($err2) {
    "WRITE-2-RESULT: REFUSED (expected) -- $($err2.Exception.GetType().FullName): $($err2.Exception.Message) [HResult=$($err2.Exception.HResult)]"
} else {
    "WRITE-2-RESULT: SUCCEEDED (NOT expected -- Windows did not block this)"
}
$after2 = Read-DrRegion -Path $drPath -Offset $p2.Offset -Length $SAMPLE_LEN
"p2-after-sha256: $(Get-Sha256Hex -Bytes $after2)"
if ((Get-Sha256Hex -Bytes $before2) -eq (Get-Sha256Hex -Bytes $after2)) {
    "WRITE-2-UNCHANGED: YES"
} else {
    "WRITE-2-UNCHANGED: NO -- the target bytes changed even though the write result was above"
}

# ── write 3: the lettered but RAW "Blank basic" partition ──────────────────
#    Distinct from write 2 on purpose: it has a drive letter and a volume
#    object (assess() refuses it as "in use by Windows" for exactly that
#    reason) but no RECOGNISED filesystem. Whether Windows' write protection
#    covers a RAW volume the same way it covers NTFS was never measured.
(New-Object System.Random(7)).NextBytes($sample)
'=== write 3: lettered RAW "Blank basic" partition (expect: refused BY WINDOWS) ==='
$before3 = Read-DrRegion -Path $drPath -Offset $p3.Offset -Length $SAMPLE_LEN
"p3-before-sha256: $(Get-Sha256Hex -Bytes $before3)"
$err3 = Write-DrRegion -Path $drPath -Offset $p3.Offset -Bytes $sample
if ($err3) {
    "WRITE-3-RESULT: REFUSED (expected) -- $($err3.Exception.GetType().FullName): $($err3.Exception.Message) [HResult=$($err3.Exception.HResult)]"
} else {
    "WRITE-3-RESULT: SUCCEEDED (NOT expected -- Windows did not block this; assess()'s ownership refusal is carrying real weight here, not redundant safety)"
}
$after3 = Read-DrRegion -Path $drPath -Offset $p3.Offset -Length $SAMPLE_LEN
"p3-after-sha256: $(Get-Sha256Hex -Bytes $after3)"
if ((Get-Sha256Hex -Bytes $before3) -eq (Get-Sha256Hex -Bytes $after3)) {
    "WRITE-3-UNCHANGED: YES"
} else {
    "WRITE-3-UNCHANGED: NO -- the target bytes changed even though the write result was above"
}

# ── final sanity: the installer's own view after everything above ──────────
'=== final survey ==='
Invoke-Installer 'survey' | Out-String -Width 200

'=== JOB COMPLETE ==='
exit 0
