# SPDX-License-Identifier: MPL-2.0
"""Encrypted video fills the fullscreen player; hidden sessions reveal only once.
Xvfb/software GPU verification, not physical Windows tray or display acceptance.
"""
import argparse
import atexit
import json
import os
import pathlib
import plistlib
import subprocess
import time
from PIL import ImageGrab, ImageStat
import rust_integration as media
from rust_reconnect_integration import setup_keys, teardown
from rust_ui_smoke import xdo


def visible_window():
    found = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', 'AirDock'],
                           text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    return found.stdout.strip().splitlines()[-1] if found.stdout.strip() else None


def wait_window(visible=True, timeout=4):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        window = visible_window()
        if bool(window) == visible:
            return window
        time.sleep(.05)
    raise AssertionError(f'Window visible={visible} not reached')


def hide(window):
    xdo('windowfocus', window)
    xdo('key', 'alt+F4')
    wait_window(False)
    time.sleep(1.2)  # Existing minimize-state check is once per second.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    binary = parser.parse_args().binary.resolve()
    output = media.ROOT/'rust-validation'/'player'
    output.mkdir(parents=True, exist_ok=True)
    wm = subprocess.Popen(['openbox'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    atexit.register(lambda: wm.terminate() if wm.poll() is None else None)
    time.sleep(.3)
    results = {}
    for case in ('fullscreen', 'fullscreen-settings', 'startup-fullscreen', 'hidden-reconnect'):
        directory = output/case
        directory.mkdir(exist_ok=True)
        (directory/'settings.json').write_text(json.dumps({'vsync': False, 'fullscreen': case=='startup-fullscreen'}))
        paths = media.generate_media(directory)
        metrics = directory/'metrics.json'
        with (directory/'receiver.log').open('w') as log:
            process = subprocess.Popen([str(binary), '--audio-null', '--port', '7016', '--config-dir', str(directory),
                                        '--metrics', str(metrics)], env=os.environ.copy(), stdout=log, stderr=log)
            connection = None
            try:
                media.wait_listener(7016, process)
                window = wait_window()
                xdo('windowfocus', window)
                time.sleep(.3)
                if case == 'fullscreen-settings':
                    xdo('mousemove', '--window', window, 402, 34)
                    xdo('click', 1)
                if case.startswith('fullscreen'):
                    xdo('key', 'F11')
                if case == 'hidden-reconnect':
                    hide(window)
                connection = media.pair.Conn('127.0.0.1', 7016)
                key = setup_keys(connection)
                if case == 'hidden-reconnect':
                    # Establish an actual audio-only stream, not just a handshake.
                    body = plistlib.dumps({'streams': [{'type': 96, 'ct': 4, 'spf': 480}]}, fmt=plistlib.FMT_BINARY)
                    assert connection.rpc('SETUP', '/stream', body, {'Content-Type': 'application/x-apple-binary-plist'})[0] == 200
                    time.sleep(1.3)
                    assert visible_window() is None, 'Audio-only session revealed the player'
                media.send_mirror(connection, key, paths[1:2])  # Portrait, intentional black side bars.
                window = wait_window()
                time.sleep(3.3 if case != 'hidden-reconnect' else .35) # Fullscreen mouse controls auto-hide.
                if case != 'hidden-reconnect':
                    capture = ImageGrab.grab(xdisplay='').convert('RGB')
                    capture.save(directory/'video-fullscreen.png')
                    width, height = capture.size
                    for region in ((0,0,180,height), (width-180,0,width,height)):
                        assert max(ImageStat.Stat(capture.crop(region)).mean) < 1., 'UI in fullscreen side bars'
                    for region in ((width//2-60,0,width//2+60,30), (width//2-60,height-30,width//2+60,height)):
                        assert max(ImageStat.Stat(capture.crop(region)).mean) > 15., 'Video does not reach screen edge'
                    xdo('key', 'Escape')
                    time.sleep(.3)
                    if case == 'fullscreen-settings':
                        capture = ImageGrab.grab(xdisplay='')
                        capture.save(directory/'settings-restored.png')
                    results[case] = {'decoded': 30, 'fullscreen_video_edges': 'passed'}
                else:
                    hide(window)
                    assert connection.rpc('FLUSH', '/stream')[0] == 200
                    media.send_mirror(connection, key, paths[1:2])
                    time.sleep(1.3)
                    assert visible_window() is None, 'Same session/FLUSH reopened a deliberately hidden window'
                    teardown(connection)
                    connection.close()
                    connection = media.pair.Conn('127.0.0.1', 7016)
                    key = setup_keys(connection)
                    media.send_mirror(connection, key, paths[1:2])
                    window = wait_window()
                    results[case] = {'audio_only_hidden': 'passed', 'same_session_flush_hidden': 'passed', 'reconnect_revealed': 'passed'}
                teardown(connection)
                connection.close()
                connection = None
                xdo('windowfocus', window)
                xdo('key', 'ctrl+q')
                assert process.wait(timeout=10) == 0
            finally:
                if connection is not None:
                    connection.close()
                if process.poll() is None:
                    process.kill()
                    process.wait()
        record = json.loads(metrics.read_text())
        assert record['decoded_frames'] == (90 if case=='hidden-reconnect' else 30), record
        assert record['uploaded_frames'] > 0, record
        results[case]['metrics'] = record
    result = {'cases': results, 'windows_tray_hardware_acceptance': 'pending'}
    (output/'results.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
