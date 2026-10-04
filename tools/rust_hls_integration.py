"""Independent synthetic iOS FCUP sender and actual FFmpeg HLS playback."""
import argparse
import json
import os
import pathlib
import plistlib
import socket
import subprocess
import threading
import time
from rust_integration import ROOT, pair, run, wait_listener


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=pathlib.Path)
    parser.add_argument('--device-backend', action='store_true', help='Exercise CPAL with an explicitly configured virtual device; not physical audio acceptance')
    parser.add_argument('--fmp4',action='store_true')
    parser.add_argument('--hdr',choices=['pq','hlg'])
    parser.add_argument('--gui',action='store_true')
    args = parser.parse_args()
    if args.hdr:
        assert args.fmp4, 'HDR fixtures use fMP4'
    output = ROOT / 'rust-validation' / (('hls-cpal' if args.device_backend else 'hls-fmp4' if args.fmp4 else 'hls-ts')+('-'+args.hdr if args.hdr else '')+('-gui' if args.gui else ''))
    output.mkdir(parents=True, exist_ok=True)
    import shutil
    fixtures=ROOT/'rust/tests/fixtures/media'/('hls-'+args.hdr if args.hdr else 'hls')
    for source in fixtures.iterdir():
        shutil.copy2(source,output/source.name)
    prefix = 'mlhls://fixture/'
    resources = {prefix + 'master.m3u8': b'#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=400000\nvideo.m3u8\n',
                 prefix + 'video.m3u8': (output / ('fmp4.m3u8' if args.fmp4 else 'video.m3u8')).read_bytes().replace(b'#EXT-X-TARGETDURATION:0', b'#EXT-X-TARGETDURATION:1')}
    resources.update({prefix + p.name: p.read_bytes() for p in output.iterdir() if p.suffix in ['.ts','.m4s','.mp4']})
    env = os.environ.copy()
    if args.device_backend:
        env.pop('AIRPLAY_AUDIO_NULL', None)
        env['RUST_LOG'] = 'info'
    else:
        env['AIRPLAY_AUDIO_NULL'] = '1'
    port = 7014
    session = 'rust-hls-fixture'
    headers = {'X-Apple-Session-ID': session, 'Content-Type': 'application/x-apple-binary-plist'}
    errors, requests = [], []
    stopped = threading.Event()
    with (output / 'receiver.log').open('w') as log:
        process = subprocess.Popen([str(args.binary.resolve()), *([] if args.gui else ['--headless']), '--hls-proxy-playback',
                                    '--port', str(port), '--config-dir', str(output / 'config'),
                                    '--metrics', str(output / 'metrics.json'), '--exit-after', '8'],
                                   env=env, stdout=log, stderr=log)
        reverse = control = None
        worker = None
        try:
            wait_listener(port, process)
            reverse = pair.Conn('127.0.0.1', port)
            assert reverse.rpc('POST', '/reverse', extra_headers=headers)[0] == 101
            reverse.sock.settimeout(.2)

            def sender():
                buffer = reverse.buf
                try:
                    while not stopped.is_set():
                        while b'\r\n\r\n' not in buffer:
                            try:
                                part = reverse.sock.recv(65536)
                            except socket.timeout:
                                if stopped.is_set():
                                    return
                                continue
                            if not part:
                                return
                            buffer += part
                        head, _, buffer = buffer.partition(b'\r\n\r\n')
                        assert head.startswith(b'POST /event HTTP/1.1')
                        length = next(int(line.split(b':', 1)[1]) for line in head.split(b'\r\n')
                                      if line.lower().startswith(b'content-length:'))
                        while len(buffer) < length:
                            buffer += reverse.sock.recv(65536)
                        body, buffer = buffer[:length], buffer[length:]
                        request = plistlib.loads(body)['request']
                        url = request['FCUP_Response_URL']
                        requests.append(url)
                        reverse.sock.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n')
                        if url not in resources:
                            continue  # The cancellation test deliberately withholds data.
                        params = {**request, 'FCUP_Response_Data': resources[url], 'FCUP_Response_StatusCode': 200}
                        action = plistlib.dumps({'type': 'unhandledURLResponse', 'params': params}, fmt=plistlib.FMT_BINARY)
                        connection = pair.Conn('127.0.0.1', port)
                        try:
                            assert connection.rpc('POST', '/action', action, headers)[0] == 200
                        finally:
                            connection.close()
                except Exception as error:
                    if not stopped.is_set():
                        errors.append(str(error))

            worker = threading.Thread(target=sender, daemon=True)
            worker.start()
            control = pair.Conn('127.0.0.1', port)
            play = plistlib.dumps({'Content-Location': prefix + 'master.m3u8'}, fmt=plistlib.FMT_BINARY)
            assert control.rpc('POST', '/play', play, headers)[0] == 200
            time.sleep(.2)
            assert control.rpc('POST', '/rate?value=0', extra_headers=headers)[0] == 200
            time.sleep(.15)
            assert control.rpc('POST', '/rate?value=1', extra_headers=headers)[0] == 200
            value = plistlib.dumps({'test': 'raw property echo'}, fmt=plistlib.FMT_BINARY)
            assert control.rpc('PUT', '/setProperty?fixture&', value, headers)[0] == 200
            assert control.rpc('PUT', '/getProperty?fixture', extra_headers=headers)[2] == value
            deadline = time.monotonic() + 4
            info = {}
            while time.monotonic() < deadline:
                info = plistlib.loads(control.rpc('GET', '/playback-info', extra_headers=headers)[2])
                if len(requests) >= 4 and info['rate'] == 0:
                    break
                time.sleep(.1)
            assert len(requests) >= 4 and not errors, (requests, errors)
            assert 0 < info['position'] <= info['duration'] + .1, info
            play = plistlib.dumps({'Content-Location': prefix + 'unanswered.m3u8'}, fmt=plistlib.FMT_BINARY)
            assert control.rpc('POST', '/play', play, headers)[0] == 200
            time.sleep(.1)
            start = time.monotonic()
            assert control.rpc('POST', '/stop', extra_headers=headers)[0] == 200
            cancel_seconds = time.monotonic() - start
            assert cancel_seconds < 3, cancel_seconds
        finally:
            stopped.set()
            if worker:
                worker.join(timeout=1)
            for connection in [reverse, control]:
                if connection:
                    connection.close()
            try:
                process.wait(timeout=12)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise
    if args.device_backend:
        log_text = (output / 'receiver.log').read_text()
        assert 'Audio output opened: 48000 Hz, 2 channels' in log_text, log_text
        assert 'Audio conversion:' not in log_text and 'Audio resampler drain:' not in log_text, log_text
    metrics = json.loads((output / 'metrics.json').read_text())
    assert metrics['decoded_frames'] == 30, metrics
    assert 40000 <= metrics['hls_audio_samples_per_channel'] <= 48000, metrics
    assert not errors, errors
    if args.gui:
        assert metrics['presented_frames']>0 and metrics['uploaded_frames']>0,metrics
        assert 'Video display:' not in (output/'receiver.log').read_text(),(output/'receiver.log').read_text()
        if args.hdr:
            assert metrics['ten_bit_uploaded_frames']>0 and metrics['hdr_uploaded_frames']>0,metrics
    result = {'fcup_requests': requests, 'decoded_frames': 30, 'audio_samples_per_channel': metrics['hls_audio_samples_per_channel'], 'playback_info': info,
              'cancellation_seconds': cancel_seconds, 'submitted_frames':metrics['presented_frames'], 'hdr':args.hdr, 'hardware_parity': 'pending'}
    (output / 'results.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
