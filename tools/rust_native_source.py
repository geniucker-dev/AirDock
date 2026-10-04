"""Read exact ABI version constants from the bundled patched FFmpeg source."""
import re
import zipfile


def expected_versions(archive):
    result = {}
    with zipfile.ZipFile(archive) as source:
        files = {name.lstrip('./'): name for name in source.namelist()}
        release = source.read(files['RELEASE']).decode().strip()
        assert release == '8.1', f'Unexpected FFmpeg source release {release}'
        for library in ('avcodec', 'avformat', 'avutil', 'swresample'):
            headers = ''.join(source.read(files[path]).decode() for path in
                              (f'lib{library}/version.h', f'lib{library}/version_major.h') if path in files)
            prefix = 'LIB' + library.upper() + '_VERSION_'
            values = [int(re.search(r'#define\s+' + prefix + component + r'\s+(\d+)', headers).group(1))
                      for component in ('MAJOR', 'MINOR', 'MICRO')]
            result[library] = (values[0] << 16) | (values[1] << 8) | values[2]
    return result
