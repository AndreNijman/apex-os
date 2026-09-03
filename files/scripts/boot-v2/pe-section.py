#!/usr/bin/env python3
"""Print one PE section's contents from a UKI.

Used by the harness to read a UKI's own sections back out of the signed
artifact rather than trusting what was passed to ukify. It is a separate file
and not an inline heredoc because the shell scripts that need it already
contain heredocs, and a nested one whose terminator collides with the outer
one silently truncates the enclosing script — which is exactly how this file
came to exist.

Sections are NUL-padded to the file alignment, so Misc_VirtualSize is the
authoritative length; reading get_data() alone appends whatever padding
followed and turns valid JSON into a parse error.
"""
import sys

import pefile


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: pe-section.py UKI.efi .sectionname", file=sys.stderr)
        return 2
    path, want = sys.argv[1], sys.argv[2].encode()
    pe = pefile.PE(path, fast_load=True)
    for section in pe.sections:
        if section.Name.rstrip(b"\x00") == want:
            data = section.get_data()[: section.Misc_VirtualSize]
            sys.stdout.write(data.rstrip(b"\x00").decode("utf-8", "replace"))
            return 0
    names = " ".join(s.Name.rstrip(b"\x00").decode() for s in pe.sections)
    print(f"{path} has no {want.decode()} section (has: {names})", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
