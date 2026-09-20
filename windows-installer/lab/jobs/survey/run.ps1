# ─────────────────────────────────────────────────────────────────────────────
#  jobs/survey — what the installer sees on a real Windows machine.
#
#  Run by the lab agent as SYSTEM, from the APEXLAB transport volume, with the
#  installer .exe sitting next to this file. Everything it prints is captured
#  into result.txt on the same volume and read back by the host.
#
#  The order is deliberate. Windows' own view comes FIRST, unprompted, so that
#  the installer's output can be compared against something this script did not
#  produce — and so that the disk numbers Windows hands out are on the record
#  for the swapped-order run to contradict.
# ─────────────────────────────────────────────────────────────────────────────
$ErrorActionPreference = 'Continue'
$exe = Join-Path $PSScriptRoot 'apex-windows-installer.exe'
if (-not (Test-Path $exe)) { "FATAL: no installer at $exe"; exit 2 }

'=== WINDOWS OWN VIEW: disks ==='
Get-Disk | Sort-Object Number |
    Format-Table Number, FriendlyName, SerialNumber, Size, PartitionStyle, BusType -AutoSize |
    Out-String -Width 200

'=== WINDOWS OWN VIEW: volumes ==='
Get-Volume | Format-Table DriveLetter, FileSystemLabel, FileSystem, Size, SizeRemaining -AutoSize |
    Out-String -Width 200

# ── The measurement ARCHITECTURE.md asks for ────────────────────────────────
#  How much room is actually left in a stock Windows ESP. The whole
#  shared-ESP design lives or dies on this number, and until now it has been
#  quoted from documentation rather than measured on a machine.
'=== THE SHARED WINDOWS ESP, MEASURED ==='
$espLetter = 'S:'
& mountvol $espLetter /S 2>&1 | Out-String
if (Test-Path $espLetter) {
    $d = Get-PSDrive -Name ($espLetter.TrimEnd(':')) -ErrorAction SilentlyContinue
    if ($d) {
        "esp-total-bytes: $($d.Used + $d.Free)"
        "esp-used-bytes : $($d.Used)"
        "esp-free-bytes : $($d.Free)"
    }
    'esp-contents:'
    Get-ChildItem -Recurse -Force $espLetter -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty FullName | Out-String -Width 200
    & mountvol $espLetter /D 2>&1 | Out-String
} else {
    'esp: could not be mounted for measurement'
}

# ── The installer's own survey ───────────────────────────────────────────────
'=== apex-windows-installer survey ==='
$survey = & $exe survey 2>&1
$survey | Out-String -Width 200
"survey-exit: $LASTEXITCODE"

# ── Every partition it listed, inspected ─────────────────────────────────────
#  The candidates come out of the tool's own output rather than being hardcoded
#  here, so this script cannot accidentally agree with itself about which
#  partitions exist.
$guids = $survey | Out-String |
    Select-String -Pattern 'PARTITION ([0-9a-f]{8}-[0-9a-f-]{27})' -AllMatches |
    ForEach-Object { $_.Matches } | ForEach-Object { $_.Groups[1].Value }

"partitions-seen: $($guids.Count)"
foreach ($g in $guids) {
    "=== inspect $g ==="
    $out = & $exe inspect $g 2>&1
    $out | Out-String -Width 200
    "inspect-exit($g): $LASTEXITCODE"
}

'=== JOB COMPLETE ==='
exit 0
