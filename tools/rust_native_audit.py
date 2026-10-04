"""Audit installed FFmpeg DLL APIs before saving the native build cache.
The final executable package is audited independently by rust_package.py.
"""
import argparse
import ctypes
import hashlib
import json
import os
import pathlib
import sys
from rust_native_source import expected_versions

LIBRARIES = {'avcodec': 62, 'avformat': 62, 'avutil': 60, 'swresample': 6}
REQUIRED = {'--disable-encoders', '--disable-muxers', '--disable-nvenc', '--enable-shared',
            '--enable-schannel', '--enable-nvdec', '--enable-cuvid'}
FORBIDDEN = {'--enable-gpl', '--enable-nonfree', '--enable-libx264', '--enable-libx265',
             '--enable-libfdk-aac', '--enable-nvenc'}


def audit(root):
    assert sys.platform == 'win32'
    root = root.resolve()
    directory = root / 'bin'
    dll_directory = os.add_dll_directory(str(directory))
    archive = root / 'share' / 'ffmpeg' / 'ffmpeg-source.zip'
    source_versions = expected_versions(archive)
    rows = {}
    libraries = {}
    for name, major in LIBRARIES.items():
        paths = list(directory.glob(name + '-*.dll'))
        assert len(paths) == 1, (name, paths)
        dll = ctypes.WinDLL(str(paths[0]), winmode=0x1100)
        libraries[name] = dll
        version = getattr(dll, name + '_version')
        version.restype = ctypes.c_uint
        license_api = getattr(dll, name + '_license')
        config_api = getattr(dll, name + '_configuration')
        license_api.restype = config_api.restype = ctypes.c_char_p
        license_text = license_api().decode()
        flags = config_api().decode().split()
        assert version() >> 16 == major
        assert version() == source_versions[name], (name, version(), source_versions[name])
        assert license_text.startswith('LGPL'), license_text
        assert REQUIRED <= set(flags), (name, REQUIRED - set(flags))
        assert not FORBIDDEN.intersection(flags), (name, flags)
        rows[name] = {'version': version(), 'license': license_text, 'configuration': ' '.join(flags),
                      'sha256': hashlib.sha256(paths[0].read_bytes()).hexdigest()}
    codec = libraries['avcodec']
    codec.av_codec_iterate.argtypes = [ctypes.POINTER(ctypes.c_void_p)]
    codec.av_codec_iterate.restype = ctypes.c_void_p
    codec.av_codec_is_encoder.argtypes = [ctypes.c_void_p]
    codec.av_codec_is_decoder.argtypes = [ctypes.c_void_p]
    state = ctypes.c_void_p()
    decoders = set()
    while pointer := codec.av_codec_iterate(ctypes.byref(state)):
        name = ctypes.cast(pointer, ctypes.POINTER(ctypes.c_char_p))[0].decode()
        assert not codec.av_codec_is_encoder(pointer), f'Unexpected encoder {name}'
        if codec.av_codec_is_decoder(pointer):
            decoders.add(name)
    assert {'h264', 'hevc', 'aac', 'alac', 'h264_cuvid', 'hevc_cuvid'} <= decoders
    archive = root / 'share' / 'ffmpeg' / 'ffmpeg-source.zip'
    assert archive.is_file() and archive.stat().st_size > 1_000_000
    result = {'dlls': rows, 'decoder_count': len(decoders), 'encoder_count': 0,
              'corresponding_source_sha256': hashlib.sha256(archive.read_bytes()).hexdigest(),
              'final_package_validation': 'performed separately', 'physical_acceptance': 'pending'}
    dll_directory.close()
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--root', type=pathlib.Path, required=True)
    p.add_argument('--output', type=pathlib.Path, required=True)
    args = p.parse_args()
    result = audit(args.root)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
