"""Fail if retired UI/audio dependencies or video encoders enter the locked Rust graph."""
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
    assert not (ROOT/'ui/app.slint').exists()
    result={'dependencies':len(names),'retired_dependencies':sorted(forbidden),'native_features':features,'project_license':'unchanged; source relationship audit required separately'}
    output=ROOT/'rust-validation';output.mkdir(exist_ok=True);(output/'dependency-audit.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))
if __name__=='__main__':main()
