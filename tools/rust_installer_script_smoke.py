"""Windows preflight for installer callbacks, before expensive native/Rust builds.

Uses a non-executable fixture payload. Full installation/playback remains a
separate gate using the actual audited application and DLLs.
"""
import argparse
import os
import pathlib
import shutil
import subprocess
import tempfile
import winreg
from rust_installer_verify import run_value, set_run, registered_uninstaller

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', type=pathlib.Path, required=True)
    args = parser.parse_args()
    output = ROOT/'rust-validation'/'installer-script'
    output.mkdir(parents=True, exist_ok=True)
    original = run_value()
    with tempfile.TemporaryDirectory(prefix='airplay-installer-preflight-', dir=os.environ['RUNNER_TEMP']) as directory:
        base = pathlib.Path(directory)
        source = base/'source'; source.mkdir()
        (source/'airplay-windows.exe').write_bytes(b'Non-executable installer fixture, never launched or distributed.')
        shutil.copy2(ROOT/'LICENSE', source/'LICENSE')
        (source/'INSTALL_PAYLOAD.txt').write_text('airplay-windows.exe\nLICENSE\nINSTALL_PAYLOAD.txt\n')
        subprocess.run([str(args.compiler), '/Qp', '/DSourceDir='+str(source), '/DOutputDir='+str(base),
                        '/DOutputName=installer-script-fixture', '/DAppVersion=0.2.0',
                        str(ROOT/'tools/installer/airplay-windows.iss')], check=True)
        install = base/'AirPlay 安装 fixture'
        def uninstall(stage):
            log = output/(stage+'.log')
            subprocess.run([str(registered_uninstaller(install)), '/VERYSILENT', '/SUPPRESSMSGBOXES',
                            '/NORESTART', '/LOG='+str(log)], check=True)
            print(log.read_text(encoding='utf-8-sig'))
        try:
            for stage, value in [('own', '"'+str(install/'airplay-windows.exe')+'" --start-hidden'),
                                 ('foreign', r'"C:\Portable Mirror\airplay-windows.exe" --start-hidden'),
                                 ('foreign_unicode', '"'+str(base/'AirPlay 测试 fixture'/'airplay-windows.exe')+'" --start-hidden')]:
                subprocess.run([str(base/'installer-script-fixture.exe'), '/VERYSILENT', '/SUPPRESSMSGBOXES',
                                '/NORESTART', '/SP-', '/DIR='+str(install)], check=True)
                set_run((value, winreg.REG_SZ))
                assert run_value() == (value, winreg.REG_SZ)
                uninstall(stage)
                assert run_value() == (None if stage == 'own' else (value, winreg.REG_SZ)), stage
            print('Installer callbacks: owned autostart removed; foreign autostart preserved.')
        finally:
            uninstaller = registered_uninstaller(install)
            if uninstaller and uninstaller.exists():
                subprocess.run([str(uninstaller), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'], check=False)
            set_run(original)


if __name__ == '__main__': main()
