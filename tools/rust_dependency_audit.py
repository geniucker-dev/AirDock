# SPDX-License-Identifier: MPL-2.0
"""Check the GUI/audio dependency policy and playback-only native profile."""
import json,pathlib,re,subprocess
ROOT=pathlib.Path(__file__).resolve().parents[1]
def main():
    metadata=json.loads(subprocess.check_output(['cargo','metadata','--locked','--format-version','1'],cwd=ROOT))
    names={p['name'] for p in metadata['packages']}
    forbidden={n for n in names if re.search(r'slint|sdl2|^rav1e$|^ravif$|x264|x265|openh264',n,re.I)}
    assert not forbidden,forbidden
    manifest=json.loads((ROOT/'rust/vcpkg.json').read_text())
    features=manifest['dependencies'][0]['features']
    assert set(features)=={'avcodec','avformat','swresample','nvcodec'},features
    assert '--disable-encoders --disable-muxers --disable-nvenc' in (ROOT/'rust/native/ports/ffmpeg/portfile.cmake').read_text()
    assert not (ROOT/'rust/src/media/recorder.rs').exists()
    result={'dependencies':len(names),'retired_dependencies':sorted(forbidden),'native_features':features,'project_license':'MPL-2.0; third-party exceptions in LICENSES.md'}
    import tomllib
    assert tomllib.loads((ROOT/'Cargo.toml').read_text())['package']['license']=='MPL-2.0'
    assert (ROOT/'LICENSE').read_text().startswith('Mozilla Public License Version 2.0')
    for source in (ROOT/'rust/src').rglob('*.rs'):
        expected='MIT' if source.name=='playfair.rs' else 'MPL-2.0'
        assert source.read_text().startswith('// SPDX-License-Identifier: '+expected),source
    assert 'SIL OPEN FONT LICENSE' in (ROOT/'rust/assets/fonts/NOTO-OFL.txt').read_text()
    assert all((ROOT/'rust/assets/fonts'/f'AirDockUICJK-{style}.ttf').exists() for style in ('Regular','SemiBold'))
    assert 'SIL OPEN FONT LICENSE' in (ROOT/'rust/assets/fonts/OFL.txt').read_text()
    assert all((ROOT/'rust/assets/fonts'/f'Manrope-{style}.ttf').exists() for style in ('Regular','SemiBold'))
    assert (ROOT/'LICENSES.md').exists() and (ROOT/'licenses/GPL-3.0.txt').exists()
    output=ROOT/'rust-validation' ;output.mkdir(exist_ok=True);(output/'dependency-audit.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))
if __name__=='__main__':main()
