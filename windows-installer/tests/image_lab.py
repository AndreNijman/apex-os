#!/usr/bin/env python3
"""Exercise the compiled CLI using only private, regular image files."""
import pathlib
import shutil
import struct
import subprocess
import tempfile
import unittest
import uuid
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[1]
EXE = ROOT / 'target/debug/apex-windows-installer'
PART = uuid.UUID('c6ce1374-7634-4cc2-9aab-4215e596c020')
LINUX = uuid.UUID('0fc63daf-8483-4772-8e79-3d69d8477de4')
DISK = uuid.UUID('aa2edb7f-b016-40f5-bc6a-52d8a677854e')
SIZE = 8 * 1024 * 1024
START = 2048
END = SIZE // 512 - 34


def fixture(kind=LINUX, attributes=0, overlap=False, stale=False,
            first_usable=34, last_usable=None, mbr_size=None):
    b = bytearray(SIZE)
    b[510:512] = b'\x55\xaa'
    b[450] = 0xee
    struct.pack_into('<II', b, 454, 1,
                     SIZE // 512 - 1 if mbr_size is None else mbr_size)
    entries = bytearray(16384)
    entries[:16] = kind.bytes_le
    entries[16:32] = PART.bytes_le
    struct.pack_into('<QQQ', entries, 32, START, END, attributes)
    name = 'APEX test target'.encode('utf-16-le')
    entries[56:56+len(name)] = name
    if overlap:
        entries[128:256] = entries[:128]
        entries[144:160] = uuid.UUID(int=17).bytes_le
    if stale:
        entries[160] = 1  # unused type GUID, residual bounds
    last = SIZE // 512 - 1
    if last_usable is None:
        last_usable = last - 33
    for current, alternate, table in [(1, last, 2), (last, 1, last-32)]:
        h = bytearray(512)
        h[:8] = b'EFI PART'
        struct.pack_into('<IIIIQQQQ', h, 8, 0x10000, 92, 0, 0,
                         current, alternate, first_usable, last_usable)
        h[56:72] = DISK.bytes_le
        struct.pack_into('<QIII', h, 72, table, 128, 128, zlib.crc32(entries))
        struct.pack_into('<I', h, 16, zlib.crc32(h[:92]))
        b[current*512:(current+1)*512] = h
        b[table*512:table*512+16384] = entries
    return b


class ImageTests(unittest.TestCase):
    def check_image(self, data, expected, selection=str(PART), success=False):
        with tempfile.TemporaryDirectory(prefix='apex-image-test-') as directory:
            path = pathlib.Path(directory) / 'disk.img'
            path.write_bytes(data)
            result = subprocess.run([str(EXE), 'lab', str(path)],
                                    input=selection+'\n', text=True,
                                    capture_output=True, timeout=30)
            self.assertEqual(result.returncode == 0, success, result.stdout+result.stderr)
            self.assertIn(expected, result.stdout+result.stderr)
            self.assertEqual(path.read_bytes(), data, 'inspector changed the image')

    def test_empty(self):
        self.check_image(fixture(), f'{(END-START+1)*512}/{(END-START+1)*512} bytes read', success=True)

    def test_windows_writes_ffffffff_as_the_protective_mbr_size(self):
        # UEFI 2.10 table 5.3: SizeInLBA is the disk size minus one, "Set to
        # 0xFFFFFFFF if the size of the disk is too large to be represented in
        # this field". Windows writes 0xFFFFFFFF ALWAYS -- measured on a
        # Windows Server 2022 install to a 40 GB disk, where the correct
        # 0x04FFFFFF fits with room to spare. Demanding the exact value refuses
        # every Windows-formatted disk in the world.
        total = (END - START + 1) * 512
        self.check_image(fixture(mbr_size=0xFFFFFFFF),
                         f'{total}/{total} bytes read', success=True)

    def test_a_one_mib_aligned_usable_range_is_accepted(self):
        # What sfdisk produces: the first partition aligned to 1 MiB, so
        # first-usable is 2048 rather than 34. Requiring exactly 34 refused
        # every disk this project's own lab builds.
        total = (END - START + 1) * 512
        self.check_image(fixture(first_usable=2048),
                         f'{total}/{total} bytes read', success=True)

    def test_a_partition_outside_the_declared_usable_range_is_refused(self):
        # The other half of the same relaxation. Widening the accepted range
        # must not stop the bounds check from using it: a partition starting
        # before the table says the usable area begins, or ending after it
        # ends, is refused.
        self.check_image(fixture(first_usable=START + 8),
                         'invalid partition bounds or identity')
        self.check_image(fixture(last_usable=END - 1),
                         'invalid partition bounds or identity')

    def test_a_usable_range_that_overruns_the_disk_is_refused(self):
        # last-usable must still lie inside the space the backup header and
        # table occupy, whatever the header claims.
        self.check_image(fixture(last_usable=SIZE // 512 - 2),
                         'unsupported GPT geometry or disagreeing backup')
        self.check_image(fixture(first_usable=33),
                         'unsupported GPT geometry or disagreeing backup')

    def test_signatures_and_arbitrary_data(self):
        for offset, signature in [(3, b'NTFS    '), (1024+56, b'\x53\xef'),
                                  (65536+64, b'_BHRfS_M'), (0, b'LUKS\xba\xbe'),
                                  (512, b'EFI PART'), (510, b'\x55\xaa'),
                                  (3*1024*1024+7, b'x'), ((END-START+1)*512-1, b'z')]:
            with self.subTest(offset=offset, signature=signature):
                data = fixture()
                absolute = START*512+offset
                data[absolute:absolute+len(signature)] = signature
                self.check_image(data, f'partition offset {offset}')

    @unittest.skipUnless(shutil.which('mkfs.ext4'), 'mkfs.ext4 unavailable')
    def test_formatted_filesystem_without_user_files(self):
        with tempfile.TemporaryDirectory(prefix='apex-ext4-test-') as directory:
            partition = pathlib.Path(directory) / 'partition.img'
            partition.write_bytes(bytes((END-START+1)*512))
            subprocess.run(['mkfs.ext4', '-q', '-F', str(partition)],
                           check=True, capture_output=True)
            b = fixture()
            b[START*512:(END+1)*512] = partition.read_bytes()
            self.check_image(b, 'NOT EMPTY')

    def test_protected_even_when_zero(self):
        for kind in ['ebd0a0a2-b9e5-4433-87c0-68b6b72699c7', # Windows basic data
                     'c12a7328-f81f-11d2-ba4b-00a0c93ec93b', # ESP
                     'de94bba4-06d1-4d40-a16a-bfd50179d6ac', # recovery
                     'e3c9e316-0b5c-4db8-817d-f92df00215ae', # MSR
                     'e6d6d379-f507-44c2-a23c-238f2a3df928']: # LVM
            with self.subTest(kind=kind):
                self.check_image(fixture(uuid.UUID(kind)), 'protected/unsupported')
        self.check_image(fixture(attributes=1), 'protected/unsupported')

    def test_selection_required(self):
        self.check_image(fixture(), 'no exact partition selected', selection='')
        self.check_image(fixture(), 'no exact partition selected', selection='0')

    def test_bad_layouts(self):
        self.check_image(fixture(overlap=True), 'overlapping')
        self.check_image(fixture(stale=True), 'stale unused')
        b = fixture(); b[512+16] ^= 1
        self.check_image(b, 'header CRC')
        b = fixture(); b[1024+56] ^= 1
        self.check_image(b, 'table CRC')
        b = fixture(); b[-512+56] ^= 1
        self.check_image(b, 'header CRC')
        b = fixture(); b[462] = 1
        self.check_image(b, 'hybrid MBR')
        self.check_image(fixture()[:-512], 'protective MBR')

    def test_neighbor_data_outside_extent_is_not_scanned(self):
        b = fixture(); b[100*512:100*512+4] = b'data'
        self.check_image(b, 'ALL-ZERO CONTENT', success=True)

    def test_symlink_refused(self):
        with tempfile.TemporaryDirectory(prefix='apex-image-test-') as directory:
            path = pathlib.Path(directory)
            (path/'real.img').write_bytes(fixture())
            (path/'link.img').symlink_to(path/'real.img')
            result = subprocess.run([str(EXE), 'lab', str(path/'link.img')], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('symlinks disabled', result.stderr)


if __name__ == '__main__':
    subprocess.run(['cargo', 'build', '--offline', '--locked'], cwd=ROOT, check=True)
    unittest.main()
