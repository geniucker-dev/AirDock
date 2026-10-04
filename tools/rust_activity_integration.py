# SPDX-License-Identifier: MPL-2.0
"""Verify receiving outlives UI presentation, and hidden/settings states stop video uploads.
Xvfb/software GPU only; this does not certify the Windows tray or physical power.
"""
import argparse
import atexit
import json
import os
import pathlib
import subprocess
import time
import rust_integration as media
from rust_reconnect_integration import setup_keys, process_cpu_seconds
from rust_ui_smoke import xdo


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    output = media.ROOT / 'rust-validation' / 'activity'
    output.mkdir(parents=True, exist_ok=True)
    wm = subprocess.Popen(['openbox'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    atexit.register(lambda: wm.terminate() if wm.poll() is None else None)
    time.sleep(.3)
    results = {}
    for state in ('receive', 'settings', 'closed', 'restored', 'software'):
        directory = output / state
        directory.mkdir(exist_ok=True)
        (directory / 'settings.json').write_text(json.dumps({'vsync': False}))
        metrics = directory / 'metrics.json'
        metrics.unlink(missing_ok=True)
        paths = media.generate_media(directory)
        with (directory / 'receiver.log').open('w') as log:
            env = os.environ.copy()
            if state == 'software': env['ICED_BACKEND'] = 'tiny-skia'
            process = subprocess.Popen([str(binary), '--audio-null', '--port', '7014', '--config-dir', str(directory),
                                        '--exit-after', '6', '--metrics', str(metrics)], stdout=log, stderr=log, env=env)
            try:
                media.wait_listener(7014, process)
                window = ''
                for _ in range(60):
                    found = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', 'AirPlay-Windows'], text=True, stdout=subprocess.PIPE)
                    if found.stdout.strip():
                        window = found.stdout.strip().splitlines()[-1]
                        break
                    time.sleep(.05)
                assert window
                xdo('windowfocus', window)
                time.sleep(.2)
                if state == 'settings':
                    xdo('mousemove', '--window', window, 402, 34)
                    xdo('click', 1)
                if state in ('closed', 'restored'):
                    xdo('key', 'alt+F4')
                time.sleep(1.2)  # Let minimize state reach the presentation controller.
                if state == 'restored':
                    restore = subprocess.run([str(binary), '--config-dir', str(directory)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=3)
                    assert restore.returncode == 0
                    time.sleep(1.2)
                before = process_cpu_seconds(process)
                time.sleep(.5)
                idle_cpu = process_cpu_seconds(process) - before
                assert idle_cpu < .2, f'{state}: idle busy loop ({idle_cpu} CPU seconds / .5s)'
                connection = media.pair.Conn('127.0.0.1', 7014)
                try:
                    key = setup_keys(connection)
                    media.send_mirror(connection, key, paths[:1])
                finally:
                    connection.close()
                assert process.wait(timeout=10) == 0
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
        record = json.loads(metrics.read_text())
        assert record['decoded_frames'] == 30, record
        if state in ('settings', 'software'):
            assert record['uploaded_frames'] == 0 and record['presented_frames'] == 0, record
        else: # A new video session restores a hidden/minimized player.
            assert record['uploaded_frames'] >= 10 and record['presented_frames'] >= 10, record
        if state == 'receive':
            assert record['ui_frame_events'] >= 10, record
            assert record['receiver_view_builds'] < record['ui_frame_events'], record
        if state == 'software':
            assert 'Video unavailable: no compatible GPU' in (directory / 'receiver.log').read_text()
        results[state] = {'decoded': record['decoded_frames'], 'uploads': record['uploaded_frames'],
                          'submissions': record['presented_frames'], 'idle_cpu_seconds_over_half_second': idle_cpu,
                          'ui_frame_events': record['ui_frame_events'], 'receiver_view_builds': record['receiver_view_builds']}
    result = {'cases': results, 'windows_tray_hardware_acceptance': 'pending', 'physical_power_comparison': 'pending'}
    (output / 'results.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
