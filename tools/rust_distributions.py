"""Build portable ZIP and user-level Inno installer from one audited payload."""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import tomllib
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def payload_files(package):
    paths = sorted(p for p in package.rglob('*') if p.is_file())
    assert paths and all(not p.is_symlink() for p in paths)
    return paths


def verify_assets(directory):
    rows = (directory/'SHA256SUMS.txt').read_text().splitlines()
    names = []
    for row in rows:
        expected, name = row.split('  ', 1)
        assert pathlib.PurePath(name).name == name and '/' not in name and '\\' not in name
        assert digest(directory/name) == expected, name
        names.append(name)
    assert len(names) == 2 and sum(n.endswith('-setup.exe') for n in names) == 1
    assert sum(n.endswith('-portable.zip') for n in names) == 1
    assert set(p.name for p in directory.iterdir()) == set(names)|{'SHA256SUMS.txt'}
    return names


def build(package, output, compiler, commit):
    package, output = package.resolve(), output.resolve()
    audit = json.loads((package/'NATIVE_AUDIT.json').read_text())
    for name, expected in audit['files'].items():
        assert digest(package/name) == expected, f'Payload changed since DLL audit: {name}'
    assert audit['runtime']['encoders'] == []
    version = tomllib.loads((ROOT/'Cargo.toml').read_text())['package']['version']
    assert re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', version)
    assert re.fullmatch(r'[0-9a-f]{40}', commit)
    output.mkdir(parents=True, exist_ok=True)
    assert not list(output.iterdir()), 'Distribution destination must be empty'
    # Installers track only their payload. AppData and untracked user files are
    # never placed in the obsolete-file cleanup list.
    files = [p.relative_to(package).as_posix() for p in payload_files(package)
             if p.name not in ('INSTALL_PAYLOAD.txt', 'PACKAGE_SHA256.json')]
    files += ['INSTALL_PAYLOAD.txt', 'PACKAGE_SHA256.json']
    (package/'INSTALL_PAYLOAD.txt').write_text('\n'.join(n.replace('/', '\\') for n in sorted(files))+'\n', encoding='utf-8')
    hashes = {p.relative_to(package).as_posix(): digest(p) for p in payload_files(package)
              if p.name != 'PACKAGE_SHA256.json'}
    (package/'PACKAGE_SHA256.json').write_text(json.dumps({'commit': commit, 'version': version, 'files': hashes}, indent=2))
    prefix = f'airplay-windows-{version}-{commit[:7]}-windows-x64'
    portable = output/(prefix+'-portable.zip')
    with zipfile.ZipFile(portable, 'w', zipfile.ZIP_DEFLATED) as archive:
        for path in payload_files(package):
            archive.write(path, path.relative_to(package).as_posix())
    with zipfile.ZipFile(portable) as archive:
        assert archive.testzip() is None
        for name, expected in hashes.items():
            assert hashlib.sha256(archive.read(name)).hexdigest() == expected, name
    subprocess.run([str(compiler), '/Qp', f'/DSourceDir={package}', f'/DOutputDir={output}',
                    f'/DOutputName={prefix}-setup', f'/DAppVersion={version}',
                    str(ROOT/'tools/installer/airplay-windows.iss')], check=True)
    setup = output/(prefix+'-setup.exe')
    assert setup.is_file() and setup.stat().st_size > 1_000_000
    (output/'SHA256SUMS.txt').write_text('\n'.join(digest(p)+'  '+p.name for p in (portable, setup))+'\n')
    verify_assets(output)
    return {'portable': portable.name, 'installer': setup.name, 'payload_files': len(hashes), 'commit': commit}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', type=pathlib.Path)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--compiler', type=pathlib.Path)
    parser.add_argument('--commit')
    parser.add_argument('--verify-only', action='store_true')
    args = parser.parse_args()
    if args.verify_only:
        print(json.dumps({'verified_assets': verify_assets(args.output)}))
    else:
        print(json.dumps(build(args.package, args.output, args.compiler, args.commit)))


if __name__ == '__main__':
    main()
