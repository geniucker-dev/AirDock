"""Real Iced interactions: draft isolation, immediate audio, persistent geometry and player controls.
Xvfb with a virtual ALSA device. Physical WASAPI/tray/DPI remain separate acceptance.
"""
import argparse
import atexit
import json
import os
import pathlib
import subprocess
import time
from PIL import ImageGrab, ImageStat
import rust_integration as media
from rust_player_integration import wait_window
from rust_ui_smoke import xdo


def until(check, timeout=5):
    deadline=time.monotonic()+timeout
    while time.monotonic()<deadline:
        if check(): return
        time.sleep(.05)
    raise AssertionError('UI change was not committed')


def geometry(window):
    return {k:int(v) for line in xdo('getwindowgeometry','--shell',window).splitlines()
            for k,v in [line.split('=',1)] if k in ('WIDTH','HEIGHT')}


def click(window,x,y):
    xdo('mousemove','--window',window,x,y);xdo('click',1);time.sleep(.1)


def edit(window,x,y,value):
    click(window,x,y);xdo('key','ctrl+a');xdo('type','--clearmodifiers','--delay',0,value)


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=pathlib.Path,required=True)
    binary=parser.parse_args().binary.resolve()
    output=media.ROOT/'rust-validation'/'preferences-ui';output.mkdir(parents=True,exist_ok=True)
    config=output/'config';config.mkdir(exist_ok=True)
    settings=config/'settings.json';settings.write_text(json.dumps({'vsync':False}))
    original=settings.read_bytes()
    alsa=output/'virtual-asound.conf'
    alsa.write_text('pcm.!default { type null hint { show on description "UI virtual output" } }\n')
    env=dict(os.environ,ALSA_CONFIG_PATH=str(alsa))
    wm=subprocess.Popen(['openbox'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    atexit.register(lambda:wm.terminate() if wm.poll() is None else None)
    time.sleep(.3)
    process=None
    def launch(name):
        log=(output/(name+'.log')).open('w')
        app=subprocess.Popen([str(binary),'--audio-null','--port','7020','--config-dir',str(config)],env=env,stdout=log,stderr=log)
        log.close();media.wait_listener(7020,app);window=wait_window();xdo('windowfocus',window);time.sleep(.3)
        return app,window
    def stored(): return json.loads(settings.read_text())
    report={}
    try:
        process,window=launch('initial')
        click(window,100,215)
        edit(window,450,205,'Unsaved receiver draft')
        assert settings.read_bytes()==original
        xdo('mousemove','--window',window,700,550);xdo('click','--repeat',3,5);time.sleep(.3)
        ImageGrab.grab(xdisplay='').save(output/'audio-scrolled.png')
        click(window,600,335)
        ImageGrab.grab(xdisplay='').save(output/'audio-menu.png')
        click(window,500,405)
        until(lambda:stored().get('audio_device','default')!='default')
        selected=stored()['audio_device'];assert stored()['name']=='AirPlay-Windows'
        report['immediate_audio_preserves_unsaved_draft']='passed'
        xdo('windowsize',window,960,640)
        until(lambda:stored().get('window_width')==960)
        assert stored()['name']=='AirPlay-Windows' and stored()['audio_device']==selected
        ImageGrab.grab(xdisplay='').save(output/'unsaved-draft-after-audio-and-resize.png')
        xdo('key','ctrl+q');assert process.wait(timeout=10)==0
        process,window=launch('restart')
        assert geometry(window)=={'WIDTH':960,'HEIGHT':640}
        click(window,100,215)
        edit(window,450,205,'Saved receiver')
        click(window,260,597)
        until(lambda:stored().get('name')=='Saved receiver')
        before=settings.read_bytes()
        edit(window,330,279,'invalid')
        click(window,260,597)
        assert settings.read_bytes()==before, 'Invalid form was partly applied'
        ImageGrab.grab(xdisplay='').save(output/'invalid-width.png')
        click(window,400,597) # Reset draft, without committing anything.
        assert settings.read_bytes()==before
        click(window,260,597)
        until(lambda:stored().get('name')=='AirPlay-Windows')
        assert stored()['audio_device']==selected
        report['validation_save_reset_and_geometry_restart']='passed'
        xdo('key','F11');time.sleep(.3)
        g=geometry(window);width,height=g['WIDTH'],g['HEIGHT']
        xdo('mousemove','--window',window,300,200);time.sleep(.3)
        capture=ImageGrab.grab(xdisplay='').convert('RGB');capture.save(output/'fullscreen-controls.png')
        assert max(ImageStat.Stat(capture.crop((0,height-90,width,height))).mean)>3
        left=max(16,(width-960)//2)
        click(window,left+960-14-260+30,height-50)
        connection=media.pair.Conn('127.0.0.1',7020)
        try:
            assert b'-144.000000' in connection.rpc('GET_PARAMETER','/stream',b'volume\r\n')[2]
            click(window,left+960-14-260+30,height-50)
            assert b'0.000000' in connection.rpc('GET_PARAMETER','/stream',b'volume\r\n')[2]
        finally: connection.close()
        xdo('mousemove','--window',window,20,20);time.sleep(3.3)
        capture=ImageGrab.grab(xdisplay='').convert('RGB');capture.save(output/'fullscreen-controls-hidden.png')
        assert max(ImageStat.Stat(capture.crop((0,height-90,width,height))).mean)<.1
        xdo('mousemove','--window',window,300,200);time.sleep(.2)
        click(window,left+74,height-50) # Exit with the mouse; restores settings.
        until(lambda:geometry(window)['WIDTH']==960)
        xdo('key','ctrl+q');assert process.wait(timeout=10)==0
        report['fullscreen_hover_mute_unmute_auto_hide_mouse_exit']='passed'
    finally:
        if process and process.poll() is None:process.kill();process.wait()
    (output/'results.json').write_text(json.dumps({'checks':report,'physical_windows_audio_tray_dpi':'pending'},indent=2))
    print(json.dumps(report,indent=2))


if __name__=='__main__':main()
