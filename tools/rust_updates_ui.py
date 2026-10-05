# SPDX-License-Identifier: MPL-2.0
"""Actual Iced update prompt/navigation and saved preferences, without network.

Installation is exercised separately against Windows distribution binaries.
The cached release here is synthetic and is never downloaded.
"""
import argparse
import atexit
import json
import pathlib
import subprocess
import time
from PIL import ImageGrab
import rust_integration as media
from rust_player_integration import wait_window
from rust_preferences_ui import click, edit, until
from rust_ui_smoke import xdo


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    binary = parser.parse_args().binary.resolve()
    output = media.ROOT/'rust-validation'/'updates-ui'
    config = output/'config'
    (config/'updates').mkdir(parents=True, exist_ok=True)
    settings = config/'settings.json'
    settings.write_text(json.dumps({'language': 'en', 'vsync': False, 'automatic_updates': False}))
    original = settings.read_bytes()
    assets = {}
    for kind, suffix in [('installer', 'setup.exe'), ('portable', 'portable.zip')]:
        name = f'airdock-0.2.0-fixture-windows-x64-{suffix}'
        assets[kind] = {'name': name, 'url': f'https://github.com/geniucker-dev/AirDock/releases/download/v0.2.0/{name}',
                        'size': 123, 'sha256': 'a'*64}
    (config/'updates/check.json').write_text(json.dumps({'checked_at': int(time.time()), 'release': {
        'version': '0.2.0', 'page': 'https://github.com/geniucker-dev/AirDock/releases/tag/v0.2.0', **assets}}))
    wm = subprocess.Popen(['openbox'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    atexit.register(lambda: wm.terminate() if wm.poll() is None else None)
    time.sleep(.3)
    process = None

    def launch(name):
        with (output/(name+'.log')).open('w') as log:
            app = subprocess.Popen([str(binary), '--audio-null', '--port', '7024', '--config-dir', str(config)], stdout=log, stderr=log)
        media.wait_listener(7024, app)
        window = wait_window()
        xdo('windowfocus', window)
        time.sleep(.3)
        return app, window

    def stored():
        return json.loads(settings.read_text())

    try:
        process, window = launch('initial')
        ImageGrab.grab(xdisplay='').save(output/'available-prompt.png')
        click(window, 1040, 737)  # View update opens settings and scrolls to its card.
        time.sleep(.3)
        ImageGrab.grab(xdisplay='').save(output/'updates-card.png')
        click(window, 685, 382)  # Enable daily checks in the draft.
        click(window, 685, 412)  # Disable mirror fallback in the draft.
        edit(window, 850, 452, 'http://invalid.example')
        click(window, 85, 675)
        assert settings.read_bytes() == original, 'Invalid mirror committed partial preferences'
        edit(window, 850, 452, 'https://gh-proxy.com, https://example.com/mirror')
        click(window, 85, 675)
        until(lambda: stored().get('automatic_updates') is True and stored().get('update_mirrors_enabled') is False)
        assert stored()['update_mirrors'] == ['https://gh-proxy.com', 'https://example.com/mirror']
        xdo('key', 'ctrl+q')
        assert process.wait(timeout=10) == 0
        saved = settings.read_bytes()
        (config/'updates/result.json').write_text(json.dumps({'success': False, 'error': 'CI fixture update rollback'}))
        process, window = launch('restart')
        assert not (config/'updates/result.json').exists(), 'Previous update receipt was not loaded'
        click(window, 1040, 737)
        time.sleep(.3)
        ImageGrab.grab(xdisplay='').save(output/'persisted-updates.png')
        assert settings.read_bytes() == saved, 'Update cache overwrote receiver settings'
        xdo('key', 'ctrl+q')
        assert process.wait(timeout=10) == 0
    finally:
        if process and process.poll() is None:
            process.kill()
            process.wait()
    report = {'cached_prompt_and_navigation': 'passed', 'invalid_mirror_draft_isolation': 'passed',
              'update_preferences_and_failure_receipt_restart': 'passed', 'windows_installation': 'separate Windows CI gate'}
    (output/'results.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
