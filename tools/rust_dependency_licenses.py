"""Ship verified license texts for the exact Windows Cargo dependency closure.

Some published crates omit their workspace's license files. Supplemental texts
are vendored from immutable source commits, never fetched during packaging.
"""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess

SUPPLEMENTS = pathlib.Path(__file__).parent / 'licenses'
PREFIXES = ('license', 'licence', 'copying', 'notice', 'copyright')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--target', default='x86_64-pc-windows-msvc')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--locked', '--format-version', '1',
        '--filter-platform', args.target]))
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    selected = {metadata['resolve']['root']}
    pending = list(selected)
    while pending:
        for dependency in nodes[pending.pop()]['dependencies']:
            if dependency not in selected:
                selected.add(dependency)
                pending.append(dependency)
    supplements = json.loads((SUPPLEMENTS / 'index.json').read_text())
    rows = []
    for package in metadata['packages']:
        if package['id'] not in selected:
            continue
        key = package['name'] + '-' + package['version']
        source = pathlib.Path(package['manifest_path']).parent
        destination = args.output / key
        destination.mkdir(exist_ok=True)
        files = [file for file in source.rglob('*') if file.is_file()
                 and file.name.lower().startswith(PREFIXES)] if package['source'] else [source / 'LICENSE']
        if package.get('license_file'):
            files.append(source / package['license_file'])
        texts = []
        for file in sorted(set(files)):
            if file.is_file():
                relative = file.relative_to(source)
                output = destination / relative
                output.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(file, output)
                texts.append({'file': relative.as_posix(),
                              'sha256': hashlib.sha256(file.read_bytes()).hexdigest(),
                              'source': 'locked Cargo package'})
        if key in supplements:
            supplement = supplements[key]
            vcs = json.loads((source / '.cargo_vcs_info.json').read_text())
            assert supplement['vcs_commit'] == vcs['git']['sha1'], key
            assert supplement['declared_license'] == package['license'], key
            for entry in supplement['files']:
                file = SUPPLEMENTS / entry['file']
                assert hashlib.sha256(file.read_bytes()).hexdigest() == entry['sha256'], key
                shutil.copy2(file, destination / entry['file'])
                texts.append(entry)
        assert texts, f'Missing license text for {key}; add an exact-source supplement'
        rows.append({'name': package['name'], 'version': package['version'],
                     'license': package.get('license'), 'repository': package.get('repository'),
                     'files': texts})
    (args.output / 'index.json').write_text(json.dumps(rows, indent=2) + '\n')
    print(f'Verified license texts for {len(rows)} packages ({args.target})')


if __name__ == '__main__':
    main()
