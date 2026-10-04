# SPDX-License-Identifier: MPL-2.0
"""Verify AirDock identity and actual Windows executable/installer resources."""
import hashlib
import struct
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def icon_payloads(path):
    data = path.read_bytes()
    reserved, kind, count = struct.unpack_from('<HHH', data)
    assert reserved == 0 and kind == 1 and count >= 7, 'Expected multi-resolution AirDock ICO'
    payloads = []
    for i in range(count):
        size, offset = struct.unpack_from('<II', data, 6 + 16 * i + 8)
        assert offset + size <= len(data), 'Truncated icon image'
        payloads.append(hashlib.sha256(data[offset:offset + size]).hexdigest())
    return set(payloads)


def audit_pe(path, *, application=False):
    import pefile
    pe = pefile.PE(str(path), fast_load=True)
    try:
        pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_RESOURCE']])
        icons = set()
        for resource in pe.DIRECTORY_ENTRY_RESOURCE.entries:
            if resource.id == 3:  # RT_ICON: compare actual embedded pixels, not a file alongside the EXE.
                for icon in resource.directory.entries:
                    for language in icon.directory.entries:
                        entry = language.data.struct
                        pixels = pe.get_data(entry.OffsetToData, entry.Size)
                        icons.add(hashlib.sha256(pixels).hexdigest())
        assert icon_payloads(ROOT / 'rust/assets/icons/airdock.ico') <= icons, 'AirDock icon missing from ' + str(path)
        strings = {}
        for entries in pe.FileInfo:
            for entry in entries:
                if entry.Key == b'StringFileInfo':
                    for table in entry.StringTable:
                        # Inno's loader reserves fixed-size version strings and
                        # pads their values with spaces. Compare their contents.
                        strings.update({k.decode(): v.decode().rstrip(' \x00') for k, v in table.entries.items()})
        assert strings['ProductName'] == 'AirDock', strings
        version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
        expected = tuple(int(n) for n in version.split('.')) + (0,)
        actual = tuple(int(n) for n in strings['ProductVersion'].split('.'))
        assert actual + (0,) * (4 - len(actual)) == expected, strings
        if application:
            assert strings['OriginalFilename'] == 'airdock.exe', strings
            assert strings['InternalName'] == 'airdock', strings
        return {'product': strings['ProductName'], 'version': strings['ProductVersion'],
                'verified_icon_images': len(icon_payloads(ROOT / 'rust/assets/icons/airdock.ico'))}
    finally:
        pe.close()
