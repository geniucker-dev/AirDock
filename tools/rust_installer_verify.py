"""Windows installation, payload integrity, upgrade, clean-path playback and uninstall.
Only CI's temporary installation is removed; real config and registry values are backed up.
"""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time
import winreg

ROOT = pathlib.Path(__file__).resolve().parent.parent
RUN = r'Software\Microsoft\Windows\CurrentVersion\Run'


def run_value():
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN, 0, winreg.KEY_READ | winreg.KEY_WOW64_64KEY) as key:
            return winreg.QueryValueEx(key, 'AirPlay-Windows')
    except FileNotFoundError:
        return None


def set_run(value):
    with winreg.CreateKeyEx(winreg.HKEY_CURRENT_USER, RUN, 0, winreg.KEY_SET_VALUE | winreg.KEY_WOW64_64KEY) as key:
        if value is None:
            try: winreg.DeleteValue(key, 'AirPlay-Windows')
            except FileNotFoundError: pass
        else: winreg.SetValueEx(key, 'AirPlay-Windows', 0, value[1], value[0])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--setup', type=pathlib.Path, required=True)
    parser.add_argument('--package', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve(); output.mkdir(parents=True, exist_ok=True)
    install = pathlib.Path(os.environ['RUNNER_TEMP'])/'AirPlay 安装 verification'
    assert not install.exists(), 'Installation test directory must be fresh'
    original_run = run_value()
    # Match directories::ProjectDirs::from("", "", "AirPlay-Windows").config_dir().
    config = pathlib.Path(os.environ['APPDATA'])/'AirPlay-Windows'/'config'
    config.mkdir(parents=True, exist_ok=True)
    sentinel = {config/'settings.json': b'{"name":"Keep user settings","recording_path":"keep user files"}',
                config/'identity.key': bytes(range(32))}
    originals = {p: p.read_bytes() if p.exists() else None for p in sentinel}
    for path, data in sentinel.items(): path.write_bytes(data)
    manifest = json.loads((args.package/'PACKAGE_SHA256.json').read_text())
    report = {}

    def preserved():
        for path, data in sentinel.items(): assert path.read_bytes() == data, f'User data changed: {path}'

    def install_and_check(stage):
        subprocess.run([str(args.setup.resolve()), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-',
                        '/DIR='+str(install), '/LOG='+str(output/(stage+'.log'))], check=True, timeout=180)
        for name, expected in manifest['files'].items():
            assert hashlib.sha256((install/name).read_bytes()).hexdigest() == expected, name
        assert (install/'PACKAGE_SHA256.json').read_bytes() == (args.package/'PACKAGE_SHA256.json').read_bytes()
        assert not (install/'obsolete-validation.dll').exists(), 'Obsolete installer-owned DLL retained'
        preserved()
        subprocess.run([sys.executable, str(ROOT/'tools/rust_package.py'), '--audit-only', '--destination', str(install)],
                       check=True, stdout=subprocess.DEVNULL)
        report[stage] = {'payload_hashes_verified': len(manifest['files']), 'dll_audit': 'passed', 'user_data_preserved': True}

    def uninstall(stage):
        subprocess.run([str(install/'unins000.exe'), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART',
                        '/LOG='+str(output/(stage+'.log'))], check=True, timeout=180)
        deadline = time.monotonic()+30
        while (install/'airplay-windows.exe').exists() and time.monotonic()<deadline: time.sleep(.1)
        assert not (install/'airplay-windows.exe').exists()
        assert not list(install.glob('*.dll')), 'Installed DLLs left after uninstall'
        assert (install/'user-note.txt').read_text() == 'untracked user file'
        preserved()
        report[stage] = {'app_removed': True, 'user_data_and_untracked_files_preserved': True}

    try:
        install_and_check('install')
        (install/'user-note.txt').write_text('untracked user file')
        (install/'obsolete-validation.dll').write_bytes(b'obsolete installer-owned DLL')
        with (install/'INSTALL_PAYLOAD.txt').open('a', encoding='utf-8') as file: file.write('obsolete-validation.dll\n')
        (install/'README.md').write_text('old application payload')
        own_run = ('"'+str(install/'airplay-windows.exe')+'" --start-hidden', winreg.REG_SZ)
        set_run(own_run)
        install_and_check('upgrade')
        assert run_value() == own_run
        assert (install/'user-note.txt').read_text() == 'untracked user file'
        env = dict(os.environ, PATH=str(install)+os.pathsep+os.environ['SystemRoot']+r'\System32'+os.pathsep+os.environ['SystemRoot'])
        for script, extra in [('rust_integration.py', ['--hdr', '--media-verifier', str(ROOT/'target/release/examples/media_verify.exe')]),
                              ('rust_hls_integration.py', ['--fmp4', '--hdr', 'pq'])]:
            subprocess.run([sys.executable, str(ROOT/'tools'/script), '--binary', str(install/'airplay-windows.exe'), *extra],
                           check=True, env=env, stdout=subprocess.DEVNULL, timeout=120)
        preserved(); report['installed_clean_path_playback'] = 'passed'
        uninstall('uninstall')
        assert run_value() is None, 'Installed application autostart entry retained'
        foreign_run = (r'"C:\Portable Mirror\airplay-windows.exe" --start-hidden', winreg.REG_SZ)
        set_run(foreign_run)
        install_and_check('reinstall')
        uninstall('uninstall_portable_autostart')
        assert run_value() == foreign_run, 'Uninstaller removed another installation autostart'
        report['physical_windows_tray_dpi_acceptance'] = 'pending'
        (output/'results.json').write_text(json.dumps(report, indent=2))
        print(json.dumps(report, indent=2))
    finally:
        if (install/'unins000.exe').exists():
            subprocess.run([str(install/'unins000.exe'), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'], check=False, timeout=180)
        set_run(original_run)
        for path, data in originals.items():
            if data is None: path.unlink(missing_ok=True)
            else: path.write_bytes(data)


if __name__ == '__main__':
    main()
