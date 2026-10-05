# SPDX-License-Identifier: MPL-2.0
"""CI-only Windows helper updates using an explicitly synthetic older payload.

Exercises actual packaged executables, installation, Unicode paths, process
waiting, clean PATH, restart, data preservation and a locked-DLL rollback.
This does not claim physical tray/iPhone verification or historical ABI coverage.
"""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
from rust_integration import wait_listener

ROOT = pathlib.Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def until(predicate, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.05)
    raise AssertionError('Timed out waiting for restarted receiver')


def run_case(package, portable, installer, base, kind, locked=False):
    app = base/'播放器 & AirDock'
    config = base/'用户配置'
    config.mkdir()
    (config/'updates').mkdir()
    settings = b'{"name":"Updater verification receiver","language":"zh-CN"}'
    (config/'settings.json').write_bytes(settings)
    (config/'identity.key').write_bytes(bytes(32))
    if kind == 'Installed':
        subprocess.run([str(installer), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', f'/DIR={app}'], check=True, timeout=120)
    else:
        shutil.copytree(package, app)
    # Represent a previous application-owned file and distinguish the old exe
    # bytes without changing its PE loader behaviour.
    manifest = json.loads((app/'PACKAGE_SHA256.json').read_text())
    manifest['version'] = '0.0.0'
    with (app/'airdock.exe').open('ab') as file:
        file.write(b'CI synthetic previous payload')
    manifest['files']['airdock.exe'] = digest(app/'airdock.exe')
    (app/'retired.dat').write_bytes(b'obsolete owned file')
    manifest['files']['retired.dat'] = digest(app/'retired.dat')
    (app/'PACKAGE_SHA256.json').write_text(json.dumps(manifest))
    with (app/'INSTALL_PAYLOAD.txt').open('a', encoding='utf-8') as file:
        file.write('\nretired.dat\n')
    (app/'personal.txt').write_bytes(b'untracked user file')
    before = digest(app/'airdock.exe')
    stage = base/'isolated-helper'
    stage.mkdir()
    for name in json.loads((package/'UPDATER_RUNTIME.json').read_text()):
        shutil.copy2(package/name, stage/name)
    version = json.loads((package/'PACKAGE_SHA256.json').read_text())['version']
    metrics = base/'restart-metrics.json'
    arguments = ['--headless', '--audio-null', '--port', '7067', '--config-dir', str(config), '--exit-after', '2', '--metrics', str(metrics)]
    payload = installer if kind == 'Installed' else portable
    payload_hash = digest(payload)
    env = {**os.environ, 'PATH': str(pathlib.Path(os.environ['SystemRoot'])/'System32')}
    with (base/'parent.log').open('w') as log:
        parent = subprocess.Popen([str(app/'airdock.exe'), '--headless', '--audio-null', '--port', '7066',
                                   '--config-dir', str(config), '--exit-after', '4'], env=env, stdout=log, stderr=log)
    wait_listener(7066, parent)  # The real receiver owns its config lock and playback DLLs.
    plan = {'schema': 1, 'package': str(payload), 'kind': kind, 'app_dir': str(app), 'config_dir': str(config),
            'parent_pid': parent.pid, 'version': version, 'sha256': payload_hash, 'arguments': arguments, 'show_window': False}
    path = stage/'plan.json'
    path.write_text(json.dumps(plan), encoding='utf-8')
    handle = None
    if locked:
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        kernel.CreateFileW.restype = wintypes.HANDLE
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        handle = kernel.CreateFileW(str(next(app.glob('avcodec-*.dll'))), 0x80000000, 1, None, 3, 0x80, None)
        assert handle != ctypes.c_void_p(-1).value, ctypes.WinError(ctypes.get_last_error())
    try:
        start = time.monotonic()
        result = subprocess.run([str(stage/'airdock-updater.exe'), '--plan', str(path)], env=env, capture_output=True, timeout=120)
        assert time.monotonic() - start >= 2, 'Helper did not wait for receiver exit'
        assert parent.wait(timeout=5) == 0
        report = json.loads((config/'updates/result.json').read_text())
        assert report['success'] == (not locked), report
        assert (result.returncode == 0) == (not locked), result.stderr
        until(metrics.is_file)
        # Metrics are written after all media/network shutdown. Allow process
        # finalization before deleting this temporary fixture on Windows.
        time.sleep(.3)
        assert (config/'settings.json').read_bytes() == settings
        assert (config/'identity.key').read_bytes() == bytes(32)
        assert (app/'personal.txt').read_bytes() == b'untracked user file'
        if locked:
            assert digest(app/'airdock.exe') == before, 'Old executable was not restored'
            assert json.loads((app/'PACKAGE_SHA256.json').read_text())['version'] == '0.0.0'
            assert (app/'retired.dat').is_file()
        else:
            assert digest(app/'airdock.exe') == digest(package/'airdock.exe')
            assert json.loads((app/'PACKAGE_SHA256.json').read_text())['version'] == version
            assert not (app/'retired.dat').exists()
        return {'result': 'passed', 'synthetic_previous_version': '0.0.0', 'target_version': version,
                'isolated_helper_runtime': 'passed', 'wait_restart_and_user_data': 'passed',
                'locked_dll_rollback': 'passed' if locked else 'not applicable'}
    finally:
        if handle is not None:
            kernel.CloseHandle(handle)
        if parent.poll() is None:
            parent.kill()
            parent.wait()
        if kind == 'Installed' and (app/'unins000.exe').exists():
            subprocess.run([str(app/'unins000.exe'), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'], check=True, timeout=60)


def main():
    assert sys.platform == 'win32', 'Use this only on an isolated Windows CI runner'
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', type=pathlib.Path, required=True)
    parser.add_argument('--distributions', type=pathlib.Path, required=True)
    args = parser.parse_args()
    package = args.package.resolve()
    distributions = args.distributions.resolve()
    portable = next(distributions.glob('*-portable.zip'))
    installer = next(distributions.glob('*-setup.exe'))
    report = {}
    for kind, locked in [('Portable', False), ('Portable', True), ('Installed', False)]:
        with tempfile.TemporaryDirectory(prefix='airdock-updater-ci-') as directory:
            report[f'{kind}-locked-{locked}'] = run_case(package, portable, installer, pathlib.Path(directory), kind, locked)
    report['physical_tray_and_iphone'] = 'pending'
    output = ROOT/'rust-validation'/'updater'
    output.mkdir(parents=True, exist_ok=True)
    (output/'results.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
