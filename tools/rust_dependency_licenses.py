"""Ship dependency license texts from the exact locked Cargo sources."""
import argparse,json,pathlib,shutil,subprocess
def main():
    p=argparse.ArgumentParser();p.add_argument('--output',type=pathlib.Path,required=True);a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
    metadata=json.loads(subprocess.check_output(['cargo','metadata','--locked','--format-version','1']))
    rows=[]
    for package in metadata['packages']:
        source=pathlib.Path(package['manifest_path']).parent;destination=a.output/(package['name']+'-'+package['version']);destination.mkdir(exist_ok=True)
        files=[f for f in source.iterdir() if f.is_file() and any(f.name.lower().startswith(n) for n in ('license','licence','copying','notice','copyright'))]
        if package.get('license_file'):files.append(source/package['license_file'])
        for file in set(files):
            if file.is_file():shutil.copy2(file,destination/file.name)
        rows.append({'name':package['name'],'version':package['version'],'license':package.get('license'),'repository':package.get('repository'),'files':[f.name for f in files]})
    (a.output/'index.json').write_text(json.dumps(rows,indent=2))
if __name__=='__main__':main()
