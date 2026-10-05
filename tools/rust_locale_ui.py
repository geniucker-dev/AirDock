# SPDX-License-Identifier: MPL-2.0
"""Actual fullscreen restart/double-click and persistent Iced language interactions.
Xvfb verification; physical Windows mixed-DPI and tray remain pending.
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


def geometry(window):
    return {k: int(v) for line in xdo('getwindowgeometry', '--shell', window).splitlines()
            for k, v in [line.split('=', 1)] if k in ('X', 'Y', 'WIDTH', 'HEIGHT')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    binary = parser.parse_args().binary.resolve()
    output = media.ROOT/'rust-validation'/'locale-ui'
    output.mkdir(parents=True, exist_ok=True)
    config = output/'config'
    config.mkdir(exist_ok=True)
    settings = config/'settings.json'
    settings.write_text(json.dumps({'vsync': False, 'language': 'en', 'fullscreen': True,
                                    'window_width': 960, 'window_height': 640}))
    wm = subprocess.Popen(['openbox'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    atexit.register(lambda: wm.terminate() if wm.poll() is None else None)
    time.sleep(.3)
    process = None
    report = {}
    screen = ImageGrab.grab(xdisplay='').size

    def stored():
        return json.loads(settings.read_text())

    def launch(name):
        with (output/(name+'.log')).open('w') as log:
            app = subprocess.Popen([str(binary), '--audio-null', '--port', '7022',
                                    '--config-dir', str(config)], stdout=log, stderr=log)
        media.wait_listener(7022, app)
        window = wait_window()
        xdo('windowfocus', window)
        time.sleep(.3)
        return app, window

    def fullscreen(window):
        g = geometry(window)
        return (g['WIDTH'], g['HEIGHT']) == screen and g['X'] == g['Y'] == 0

    def normal(window, width=960, height=640):
        g = geometry(window)
        return g['WIDTH'] == width and g['HEIGHT'] == height and g['X'] >= 0 and g['Y'] >= 0 and g['X']+width <= screen[0] and g['Y']+height <= screen[1]

    def capture(name):
        ImageGrab.grab(xdisplay='').save(output/(name+'.png'))

    try:
        process, window = launch('startup-fullscreen')
        until(lambda: fullscreen(window))
        capture('startup-fullscreen')
        assert stored()['window_width'] == 960 and stored()['window_height'] == 640
        xdo('key', 'Escape')
        until(lambda: normal(window))
        until(lambda: not stored()['fullscreen'])
        # Two real pointer presses toggle the video viewport, in both modes.
        for expected in (True, False):
            xdo('mousemove', '--window', window, 400, 300)
            xdo('click', '--repeat', 2, '--delay', 80, 1)
            until(lambda: fullscreen(window) if expected else normal(window))
            until(lambda: stored()['fullscreen'] == expected)
            time.sleep(.5)  # Distinguish the next double-click from this click pair.
        assert 'Window mode could not be applied' not in (output/'startup-fullscreen.log').read_text()
        report['fullscreen_restart_normal_bounds_and_double_click'] = 'passed'
        xdo('windowsize', window, 1120, 760)
        until(lambda: stored().get('window_width') == 1120)
        click(window, 402, 34)
        edit(window, 300, 248, 'Language draft')
        assert stored()['name'] == 'AirDock'
        capture('english-unsaved')
        click(window, 850, 460)
        capture('language-menu')
        click(window, 850, 422)
        until(lambda: stored().get('language') == 'zh-CN')
        assert stored()['name'] == 'AirDock', 'Language selection committed an unrelated draft'
        capture('chinese-unsaved')
        click(window, 85, 717)
        until(lambda: stored()['name'] == 'Language draft')
        report['immediate_language_preserves_unsaved_draft'] = 'passed'
        xdo('key', 'ctrl+q')
        assert process.wait(timeout=10) == 0
        process, window = launch('chinese-restart')
        until(lambda: normal(window, 1120, 760))
        capture('chinese-receiver-restart')
        click(window, 1013, 735)
        capture('chinese-diagnostics')
        click(window, 402, 34)
        capture('chinese-settings-restart')
        # Switching back must update/persist, including after a restart.
        click(window, 850, 460)
        click(window, 850, 385)
        until(lambda: stored().get('language') == 'en')
        assert stored()['name'] == 'Language draft'
        capture('english-restored')
        report['language_restart_and_switch_back'] = 'passed'
        xdo('key', 'ctrl+q')
        assert process.wait(timeout=10) == 0
    finally:
        if process and process.poll() is None:
            process.kill()
            process.wait()
    (output/'results.json').write_text(json.dumps({'checks': report, 'windows_mixed_dpi_tray': 'pending'}, indent=2))
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
