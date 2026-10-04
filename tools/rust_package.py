# SPDX-License-Identifier: MPL-2.0
"""Package only the verified runtime DLL closure and audit the DLLs actually loaded.

Requires pefile. Source/build material is mandatory for Windows distributions.
The system fixture generator is deliberately outside the distribution.
"""
import argparse,ctypes,hashlib,json,os,pathlib,re,shutil,subprocess,sys
from rust_native_source import expected_versions
FORBIDDEN=re.compile(r'(sdl|slint|x264|x265|nvenc|fdk[-_]?aac|avdevice|avfilter|postproc)',re.I)
LIBRARIES={'avcodec':62,'avformat':62,'avutil':60,'swresample':6}
def imports(path):
    import pefile
    pe=pefile.PE(str(path),fast_load=True)
    pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_IMPORT'],pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT']])
    names=[entry.dll.decode() for table in ['DIRECTORY_ENTRY_IMPORT','DIRECTORY_ENTRY_DELAY_IMPORT'] for entry in getattr(pe,table,[])]
    pe.close();return names

def build_package(binary,native,crt,destination):
    if destination.exists():shutil.rmtree(destination)
    destination.mkdir(parents=True)
    shutil.copy2(binary,destination/binary.name)
    roots=[native,crt];pending=[destination/binary.name];seen=set()
    system=pathlib.Path(os.environ['SystemRoot'])/'System32'
    while pending:
        path=pending.pop()
        for name in imports(path):
            lower=name.lower()
            if lower in seen:continue
            seen.add(lower)
            assert not FORBIDDEN.search(name),f'Forbidden runtime dependency: {name}'
            source=next((p for root in roots for p in root.glob('*') if p.name.lower()==lower),None)
            if source:
                shutil.copy2(source,destination/source.name);pending.append(destination/source.name)
            else:
                assert lower.startswith(('api-ms-win-','ext-ms-win-')) or (system/name).exists(),f'Unresolved dependency: {name}'
    return seen

def audit(directory):
    assert sys.platform=='win32','DLL audit must execute on Windows'
    directory=directory.resolve()
    assert all(not FORBIDDEN.search(p.name) for p in directory.glob('*.dll')),'Forbidden DLL in package'
    # LoadLibrary search includes this directory and Windows system DLLs, not the build PATH.
    os.environ['PATH']=str(directory)+os.pathsep+str(pathlib.Path(os.environ['SystemRoot'])/'System32')
    handle=os.add_dll_directory(str(directory))
    report=json.loads(subprocess.check_output([str(directory/'airplay-windows.exe'),'--native-report'],cwd=directory,text=True))
    kernel=ctypes.WinDLL('kernel32',use_last_error=True)
    kernel.GetModuleFileNameW.argtypes=[ctypes.c_void_p,ctypes.c_wchar_p,ctypes.c_uint32]
    source_versions=expected_versions(directory/'sources/ffmpeg-source.zip')
    rows={}
    for library,major in LIBRARIES.items():
        candidates=list(directory.glob(library+'-*.dll'));assert len(candidates)==1,(library,candidates)
        path=candidates[0];dll=ctypes.WinDLL(str(path),winmode=0x1100)
        loaded=ctypes.create_unicode_buffer(32768)
        assert kernel.GetModuleFileNameW(dll._handle,loaded,len(loaded)),ctypes.WinError(ctypes.get_last_error())
        assert pathlib.Path(loaded.value).resolve()==path.resolve(),f'Wrong loaded DLL: {loaded.value}'
        entry=report[library];assert entry['version']>>16==major,(library,entry['version'])
        assert entry['version']==source_versions[library],(library,entry['version'],source_versions[library])
        assert entry['license'].startswith('LGPL'),(library,entry['license'])
        flags=entry['configuration'].split()
        assert not any(flag in ['--enable-gpl','--enable-nonfree','--enable-libx264','--enable-libx265','--enable-libfdk-aac','--enable-nvenc'] for flag in flags),entry
        for required in ['--disable-encoders','--disable-muxers','--enable-shared','--disable-nvenc','--enable-schannel','--enable-nvdec','--enable-cuvid']:
            assert required in flags,(library,'Missing build flag',required)
        rows[library]={**entry,'dll':path.name,'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'loaded_path':loaded.value,'imports':imports(path)}
    assert not report['encoders'],report['encoders']
    assert {'h264','hevc','aac','alac','h264_cuvid','hevc_cuvid'}<=set(report['decoders']),report['decoders']
    assert {'hls','mov','mpegts'}<=set(','.join(report['demuxers']).split(',')),report['demuxers']
    assert {'http','https','tls','tcp','crypto'}<=set(report['input_protocols']),report['input_protocols']
    for path in directory.glob('*.dll'):
        for name in imports(path):
            assert not FORBIDDEN.search(name),name
            assert (directory/name).exists() or name.lower().startswith(('api-ms-win-','ext-ms-win-')) or (pathlib.Path(os.environ['SystemRoot'])/'System32'/name).exists(),name
    source_archive=directory/'sources/airplay-windows-source.zip'
    assert source_archive.exists(),'Exact application corresponding source archive missing'
    import zipfile,tomllib
    assert (directory/'LICENSE').read_text().startswith('Mozilla Public License Version 2.0'), 'Wrong project license in payload'
    assert (directory/'LICENSES.md').exists() and (directory/'licenses/GPL-3.0.txt').exists(), 'Missing license scope or retained GPL reference notices'
    with zipfile.ZipFile(source_archive) as source:
        assert source.read('rust/assets/fonts/OFL.txt')==(directory/'MANROPE_LICENSE.txt').read_bytes(), 'Missing or mismatched embedded font license'
        assert source.read('LICENSE')==(directory/'LICENSE').read_bytes(),'Payload/source license mismatch'
        assert tomllib.loads(source.read('Cargo.toml').decode())['package']['license']=='MPL-2.0','Wrong archived manifest license'
        assert source.read('rust/src/desktop.rs').startswith(b'// SPDX-License-Identifier: MPL-2.0'),'Source predates relicensing'
        assert source.read('rust/src/playfair.rs').startswith(b'// SPDX-License-Identifier: MIT'),'Third-party license was overwritten'

    assert (directory/'sources/native-triplets/x64-windows.cmake').exists(),'Native build triplet missing'
    assert (directory/'sources/ffmpeg-source.zip').exists(),'Corresponding patched FFmpeg source archive missing'
    assert (directory/'sources/native-build/ffmpeg/portfile.cmake').exists(),'Native build materials missing'
    notices=json.loads((directory/'rust-licenses/index.json').read_text())
    assert notices, 'Rust dependency license index missing'
    for crate in notices:
        assert crate['files'], f"Missing license texts: {crate['name']}"
        base=directory/'rust-licenses'/(crate['name']+'-'+crate['version'])
        for entry in crate['files']:
            assert hashlib.sha256((base/entry['file']).read_bytes()).hexdigest()==entry['sha256'], crate['name']
    result={'dlls':rows,'runtime':report,'files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in directory.iterdir() if p.suffix in ['.dll','.exe']},'verified_rust_license_packages':len(notices),'physical_windows_iphone_acceptance':'pending'}
    (directory/'NATIVE_AUDIT.json').write_text(json.dumps(result,indent=2))
    handle.close();return result

def main():
    p=argparse.ArgumentParser();p.add_argument('--binary',type=pathlib.Path);p.add_argument('--native',type=pathlib.Path);p.add_argument('--crt',type=pathlib.Path);p.add_argument('--destination',type=pathlib.Path,required=True);p.add_argument('--audit-only',action='store_true');args=p.parse_args()
    if not args.audit_only:build_package(args.binary,args.native,args.crt,args.destination)
    else:print(json.dumps(audit(args.destination),indent=2))
if __name__=='__main__':main()
