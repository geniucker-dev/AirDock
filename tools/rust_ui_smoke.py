"""Exercise Iced pages, fullscreen/focus restoration and legacy config upgrade in Xvfb.
Windows DPI, tray and physical GPU acceptance remain separate hardware checks.
"""
import atexit
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import time
from rust_integration import ROOT, wait_listener
from PIL import Image, ImageChops, ImageStat


def xdo(*args):
    return subprocess.check_output(['xdotool', *map(str, args)], text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    args = parser.parse_args()
    output = ROOT / 'rust-validation' / 'ui'
    output.mkdir(parents=True, exist_ok=True)
    wm = subprocess.Popen(["openbox"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    atexit.register(lambda: wm.terminate() if wm.poll() is None else None)
    time.sleep(.25)
    captures = {}
    for page, y in [('receiver', None), ('settings', 215), ('diagnostics', 260),
                    ('focus', None), ('fullscreen', None), ('restore', None),
                    ('fullscreen-settings', 215), ('restore-settings', 215),
                    ('edit-settings', 215), ('resize', None)]:
        directory = output / page
        directory.mkdir(exist_ok=True)
        legacy = {'vsync': False, 'recording_enabled': True, 'recording_path': 'preserve-existing-files'}
        settings = directory / 'settings.json'
        settings.write_text(json.dumps(legacy))
        original = settings.read_bytes()
        capture = output / (page + '.png')
        capture.unlink(missing_ok=True)
        env = dict(os.environ, AIRPLAY_AUDIO_NULL='1')
        with (directory / 'receiver.log').open('w') as log:
            process = subprocess.Popen([str(args.binary.resolve()), '--port', '7012', '--config-dir', str(directory),
                                        '--exit-after', '6', '--screenshot', str(capture)], env=env, stdout=log, stderr=log)
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
                assert windows, 'Iced window did not appear'
                window = windows[-1]
                xdo('windowfocus', window)
                time.sleep(.25)
                if y is not None:
                    xdo('mousemove', '--window', window, 100, y)
                    xdo('click', 1)
                if page in ['focus', 'fullscreen']:
                    xdo('key', 'ctrl+h' if page == 'focus' else 'F11')
                if page in ['fullscreen-settings', 'restore-settings']:
                    xdo('key', 'F11')
                    if page == 'restore-settings':
                        time.sleep(.2)
                        xdo('key', 'Escape')
                if page == 'restore':
                    for key in ['ctrl+h', 'F11', 'Escape', 'ctrl+h']:
                        xdo('key', key)
                        time.sleep(.1)
                if page == 'resize':
                    xdo('windowsize', window, 960, 640)
                if page == 'edit-settings':
                    time.sleep(.15)
                    xdo('mousemove', '--window', window, 450, 205)
                    xdo('click', 1)
                    xdo('key', 'ctrl+a')
                    xdo('type', '--clearmodifiers', '--delay', 0, 'Iced UI acceptance')
                    xdo('mousemove', '--window', window, 260, 717)
                    xdo('click', 1)
                assert process.wait(timeout=12) == 0, directory
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
        png = capture.read_bytes()
        assert png[:8] == b'\x89PNG\r\n\x1a\n' and len(png) > 1000, capture
        with Image.open(capture) as image:
            width, height = image.size
            viewport = image.crop((210, 75, min(width, 1095), min(height, 680))).convert('RGBA').tobytes()
            if page in ['fullscreen', 'fullscreen-settings']:
                # The screen edges belong to the player, with no navigation,
                # title, footer or outer padding even when entering from settings.
                for region in [(0, 0, 180, 180), (0, height-70, width, height)]:
                    assert max(ImageStat.Stat(image.crop(region).convert('RGB')).mean) < .1, page
        captures[page] = {'width': width, 'height': height, 'sha256': hashlib.sha256(png).hexdigest(),
                          'viewport_sha256': hashlib.sha256(viewport).hexdigest()}
        if page == 'edit-settings':
            saved = json.loads(settings.read_text())
            assert saved['name'] == 'Iced UI acceptance', saved
            assert 'recording_enabled' not in saved and 'recording_path' not in saved
        elif page in ['receiver','settings','diagnostics']:
            assert settings.read_bytes() == original, 'Reading legacy config modified the user file'
        else:
            saved=json.loads(settings.read_text())
            assert saved['name']=='AirPlay-Windows' and saved['vsync'] is False
            if page=='resize': assert (saved['window_width'],saved['window_height'])==(960,640),saved
            if page.startswith('fullscreen'): assert saved['fullscreen'] is True,saved
    assert captures['fullscreen']['width'] > captures['receiver']['width']
    assert captures['fullscreen-settings']['width'] == captures['fullscreen']['width']
    assert captures['resize']['width'] == 960
    assert len({captures[p]['viewport_sha256'] for p in ['receiver', 'settings', 'diagnostics', 'focus']}) == 4
    assert (captures['restore']['width'], captures['restore']['height']) == (captures['receiver']['width'], captures['receiver']['height'])
    # Rasterized glyph edges may differ slightly after a GPU atlas rebuild.
    # Verify restored layout/content with a tight pixel bound, not exact glyph hashes.
    with Image.open(output / 'receiver.png') as before, Image.open(output / 'restore.png') as after:
        difference = ImageChops.difference(before.convert('RGB'), after.convert('RGB'))
        mean_error = max(ImageStat.Stat(difference).mean)
    assert mean_error < 1.0, f'Restored scene differs: mean RGB error {mean_error}/255'
    captures['restore']['mean_error_255'] = mean_error
    with Image.open(output/'settings.png') as before, Image.open(output/'restore-settings.png') as after:
        assert before.size == after.size
        mean_error = max(ImageStat.Stat(ImageChops.difference(before.convert('RGB'), after.convert('RGB'))).mean)
        assert mean_error < 1.0, f'Settings page not restored: {mean_error}/255'
    captures['restore-settings']['mean_error_255'] = mean_error
    result = {'pages_and_restore': captures, 'windows_tray_dpi_hardware_acceptance': 'pending'}
    (output / 'results.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
