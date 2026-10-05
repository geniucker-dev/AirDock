"""Generate synthetic Main10/PQ/HLG fixtures with external FFmpeg tools only.

This script is never run by the receiver or distribution CI. Its encoders are
not application dependencies. Refresh and review checked-in fixtures explicitly.
"""
import argparse
import hashlib
import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ffmpeg', default='ffmpeg')
    parser.add_argument('--ffprobe', default='ffprobe')
    args = parser.parse_args()
    root = ROOT / 'rust/tests/fixtures/media'
    for mode, size, primaries, trc, matrix in [
        ('sdr', '96x128', 'bt709', 'bt709', 'bt709'),
        ('pq', '128x96', 'bt2020', 'smpte2084', 'bt2020nc'),
        ('hlg', '128x96', 'bt2020', 'arib-std-b67', 'bt2020nc'),
    ]:
        path = root / f'hevc-main10-{mode}.mp4'
        params = 'pools=none:frame-threads=1:keyint=15:min-keyint=15:scenecut=0:bframes=0:repeat-headers=1'
        params += ':colorprim=1:transfer=1:colormatrix=1' if mode == 'sdr' else ':colorprim=9:transfer='+('16' if mode == 'pq' else '18')+':colormatrix=9'
        if mode == 'pq':
            params += ':hdr10=1:master-display=G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1):max-cll=800,200'
        subprocess.run([args.ffmpeg, '-hide_banner', '-loglevel', 'error', '-y',
                        '-f', 'lavfi', '-i', f'testsrc2=size={size}:rate=30',
                        '-frames:v', '30', '-pix_fmt', 'yuv420p10le', '-c:v', 'libx265',
                        '-preset', 'fast', '-x265-params', params, '-color_range', 'tv',
                        '-color_primaries', primaries, '-color_trc', trc, '-colorspace', matrix,
                        str(path)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        probe = subprocess.check_output([args.ffprobe, '-v', 'error', '-show_streams',
                                         '-show_packets', '-show_data', '-of', 'json', str(path)])
        path.with_suffix('.json').write_bytes(probe)
        if mode != 'sdr':
            directory = root / ('hls-' + mode)
            directory.mkdir(exist_ok=True)
            subprocess.run([args.ffmpeg, '-hide_banner', '-loglevel', 'error', '-y',
                            '-i', str(path), '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=44100',
                            '-map', '0:v', '-map', '1:a', '-t', '1', '-c:v', 'copy', '-c:a', 'aac',
                            '-f', 'hls', '-hls_time', '0.5', '-hls_playlist_type', 'vod',
                            '-hls_segment_type', 'fmp4', '-hls_fmp4_init_filename', 'init.mp4',
                            '-hls_segment_filename', str(directory / 'fragment%03d.m4s'),
                            str(directory / 'fmp4.m3u8')], check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    version = subprocess.check_output([args.ffmpeg, '-version'], text=True).splitlines()[0]
    (root / 'main10-generator.json').write_text(json.dumps({
        'tool': version, 'content': 'Synthetic testsrc2 and sine, no device recording',
        'profiles': ['Main10 SDR BT.709', 'PQ BT.2020 MaxCLL 800 mastering peak 1000',
                     'HLG BT.2020 nominal peak 1000'],
    }, indent=2) + '\n')
    hashes = {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in sorted(root.rglob('*')) if p.is_file() and p.name != 'sha256.json'}
    (root / 'sha256.json').write_text(json.dumps(hashes, indent=2) + '\n')
    print(f'Generated three Main10 fixtures and PQ/HLG fMP4, pinned {len(hashes)} files')


if __name__ == '__main__':
    main()
