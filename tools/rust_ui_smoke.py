"""Exercise Slint pages and UI/fullscreen restoration under xvfb on Linux."""
import argparse
import hashlib
import json
import os
import pathlib
import struct
import subprocess
import time
from rust_integration import ROOT, wait_listener
from PIL import Image


def xdo(*args):
    return subprocess.check_output(['xdotool', *map(str, args)], text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    args = parser.parse_args()
    output = ROOT / 'rust-validation' / 'ui'
    output.mkdir(parents=True, exist_ok=True)
    captures = {}
    for page, y in [('receiver', None), ('settings', 206), ('recording', 154),
                    ('about', 258), ('hidden', None), ('fullscreen', None),
                    ('restore', None), ('edit-settings', 206)]:
        directory = output / page
        directory.mkdir(exist_ok=True)
        (directory / 'settings.json').write_text(json.dumps({'vsync': False}))
        capture = output / (page + '.png')
        capture.unlink(missing_ok=True)
        env = os.environ.copy()
        env['SDL_AUDIODRIVER'] = 'dummy'
        with (directory / 'receiver.log').open('w') as log:
            process = subprocess.Popen([
                str(args.binary.resolve()), '--port', '7012', '--config-dir', str(directory),
                '--exit-after', '3', '--screenshot', str(capture)
            ], env=env, stdout=log, stderr=log)
            try:
                wait_listener(7012, process)
                deadline = time.monotonic() + 5
                windows = []
                while not windows and time.monotonic() < deadline:
                    found = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', 'AirPlay-Windows'],
                                           text=True, stdout=subprocess.PIPE, check=False)
                    windows = found.stdout.strip().splitlines()
                    if not windows:
                        time.sleep(.025)
                assert windows, 'Slint window did not appear'
                window = windows[-1]
                xdo('windowfocus', window)
                # Wait for the first scene/layout before clicking the navigation.
                time.sleep(.25)
                if y is not None:
                    xdo('mousemove', '--window', window, 100, y)
                    xdo('click', 1)
                if page in ['hidden', 'fullscreen']:
                    xdo('key', 'ctrl+h' if page == 'hidden' else 'F11')
                if page == 'edit-settings':
                    time.sleep(.1)
                    xdo('mousemove', '--window', window, 500, 219)
                    xdo('click', 1)
                    xdo('key', 'ctrl+a')
                    xdo('type', '--clearmodifiers', '--delay', 0, 'Rust UI acceptance')
                    xdo('mousemove', '--window', window, 280, 322)
                    xdo('click', 1)
                    xdo('mousemove', '--window', window, 450, 678)
                    xdo('click', 1)
                if page == 'restore':
                    for key in ['ctrl+h', 'F11', 'Escape', 'ctrl+h']:
                        xdo('key', key)
                        time.sleep(.1)
                assert process.wait(timeout=10) == 0, directory
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
        png = capture.read_bytes()
        assert png[:8] == b'\x89PNG\r\n\x1a\n', capture
        width, height = struct.unpack('>II', png[16:24])
        assert width >= 900 and height >= 620 and len(png) > (100 if page == 'hidden' else 10000), capture
        with Image.open(capture) as image:
            if page == 'hidden':
                assert image.convert('RGB').getextrema() == ((10, 10), (13, 13), (19, 19)), 'Hidden UI retained visible pixels'
            viewport = image.crop((270, 100, 1100, 690)).convert('RGBA').tobytes()
        captures[page] = {'width': width, 'height': height,
                          'sha256': hashlib.sha256(png).hexdigest(),
                          'viewport_sha256': hashlib.sha256(viewport).hexdigest()}
        if page == 'edit-settings':
            saved = json.loads((directory / 'settings.json').read_text())
            assert saved['name'] == 'Rust UI acceptance' and not saved['hevc_enabled'], saved
    assert captures['fullscreen']['width'] > captures['receiver']['width']
    assert len({captures[p]['viewport_sha256'] for p in ['receiver', 'settings', 'recording', 'about']}) == 4
    assert captures['restore']['viewport_sha256'] == captures['receiver']['viewport_sha256']
    result = {'pages_and_restore': captures, 'windows_tray_hardware_acceptance': 'pending'}
    (output / 'results.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
