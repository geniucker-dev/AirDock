# SPDX-License-Identifier: MPL-2.0
"""Isolate encrypted mirror/RAOP decode and scheduling from UI and GPU costs.

Linux process CPU and private resident memory only; null audio is not WASAPI
acceptance. Use an H.264/HEVC 60 fps MP4 fixture without reordered frames.
"""
import argparse
import json
import os
import pathlib
import platform
import signal
import struct
import subprocess
import time

import rust_release_benchmark as bench


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--release', type=pathlib.Path, required=True)
    parser.add_argument('--current', type=pathlib.Path, required=True)
    parser.add_argument('--release-commit', required=True)
    parser.add_argument('--current-commit', required=True)
    parser.add_argument('--fixture', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--runs', type=int, default=3)
    parser.add_argument('--seconds', type=int, default=30)
    parser.add_argument('--warmup', type=int, default=3)
    parser.add_argument('--port', type=int, default=7021)
    args = parser.parse_args()
    assert platform.system() == 'Linux', 'Linux process accounting is required'
    assert args.runs >= 1 and args.seconds >= 1 and args.warmup >= 1
    probe = json.loads(bench.command('ffprobe', '-v', 'error', '-select_streams', 'v', '-show_streams',
                                    '-show_packets', '-show_data', '-of', 'json', args.fixture))
    info = probe['streams'][0]
    assert info['codec_name'] in ('h264', 'hevc') and info['r_frame_rate'] == '60/1', info
    assert info['has_b_frames'] == 0 and 'K' in probe['packets'][0]['flags'], info
    config = bench.media.ffmpeg_hex(info['extradata'])
    if info['codec_name'] == 'hevc':
        box = struct.pack('>I', len(config) + 8) + b'hvcC' + config
        config = struct.pack('>I', 86 + len(box)) + b'hvc1' + bytes(78) + box
    clip = (config, [(bench.media.ffmpeg_hex(p['data']), 'K' in p['flags']) for p in probe['packets']])
    binaries = {'release': args.release.resolve(), 'current': args.current.resolve()}
    args.output.mkdir(parents=True, exist_ok=True)
    result = {'measurement': 'Encrypted mirroring plus AAC-ELD; headless, software decode and null audio',
              'physical_hardware': False, 'release_commit': args.release_commit, 'current_commit': args.current_commit,
              'input': {'name': args.fixture.name, 'sha256': bench.sha256(args.fixture),
                        'codec': info['codec_name'], 'width': info['width'], 'height': info['height'], 'fps': 60},
              'binary_sha256': {k: bench.sha256(p) for k, p in binaries.items()},
              'harness_sha256': bench.sha256(pathlib.Path(__file__)),
              'shared_harness_sha256': bench.sha256(pathlib.Path(bench.__file__)),
              'warmup_seconds': args.warmup, 'measured_seconds': args.seconds, 'runs': []}
    for index in range(args.runs):
        order = ('release', 'current') if index % 2 == 0 else ('current', 'release')
        for version in order:
            directory = args.output / f'{index}-{version}'
            directory.mkdir(exist_ok=True)
            (directory / 'settings.json').write_text(json.dumps({'hardware_decode': False, 'automatic_updates': False}))
            with (directory / 'receiver.log').open('w') as log:
                process = subprocess.Popen([str(binaries[version]), '--headless', '--audio-null', '--port', str(args.port),
                                            '--config-dir', str(directory), '--metrics', str(directory / 'metrics.json')],
                                           env=dict(os.environ, RUST_LOG='warn'), stdout=log, stderr=log)
                control = audio = mirror = None
                try:
                    bench.media.wait_listener(args.port, process)
                    control = bench.media.pair.Conn('127.0.0.1', args.port)
                    key = bench.setup_keys(control)
                    audio = bench.Audio(control, key)
                    mirror = bench.Mirror(control, key, clip)
                    mirror.play(args.warmup)
                    with bench.Sample(process) as sample:
                        mirror.play(args.seconds)
                    resources = sample.result()
                    resources['sent_video_frames'] = mirror.sent
                    mirror.close()
                    mirror = None
                    audio.close()
                    resources['sent_audio_packets'] = audio.sent
                    audio = None
                    time.sleep(.3)
                    process.send_signal(signal.SIGINT)
                    assert process.wait(timeout=15) == 0
                finally:
                    if audio:
                        audio.close()
                    if mirror:
                        mirror.close()
                    if control:
                        control.close()
                    if process.poll() is None:
                        process.kill()
                        process.wait()
            metrics = json.loads((directory / 'metrics.json').read_text())
            assert metrics['decoded_frames'] == resources['sent_video_frames'], metrics
            assert metrics['audio_packets'] == resources['sent_audio_packets'] and metrics['audio_errors'] == 0, metrics
            assert metrics['presented_frames'] == metrics['uploaded_frames'] == 0, metrics
            record = {'version': version, 'case': info['codec_name'] + '-headless', 'run': index,
                      'metrics': metrics, **resources}
            result['runs'].append(record)
            (args.output / 'results.json').write_text(json.dumps(result, indent=2))
            print(json.dumps({k: record[k] for k in ('version', 'run', 'receiver_cpu_percent_one_core',
                                                   'private_resident_mib_median')}), flush=True)


if __name__ == '__main__':
    main()
