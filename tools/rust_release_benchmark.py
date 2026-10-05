# SPDX-License-Identifier: MPL-2.0
"""Compare two optimized receivers on Linux/X11; never certifies hardware parity.

Run under Xvfb with a window manager and the same Vulkan driver for both binaries.
ffmpeg/ffprobe are development fixture generators, not application dependencies.
CPU is receiver process time (100% = one core); memory is private resident memory.
Frames and timings describe new GPU submissions, not physical display events.
"""
import argparse
import contextlib
import hashlib
import itertools
import json
import os
import pathlib
import platform
import plistlib
import signal
import socket
import statistics
import struct
import subprocess
import threading
import time

from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
import rust_integration as media
from rust_reconnect_integration import process_cpu_seconds, setup_keys


def sha256(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def command(*args):
    return subprocess.check_output(list(map(str, args)), stderr=subprocess.PIPE)


def xdo(*args):
    return command('xdotool', *args).decode().strip()


def window(process):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError('Receiver exited before its window appeared')
        result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(process.pid),
                                 '--name', 'AirDock'], capture_output=True, text=True)
        if result.stdout.strip():
            return result.stdout.strip().splitlines()[-1]
        time.sleep(.05)
    raise TimeoutError('Receiver window did not appear')


def private_memory(process):
    values = {}
    for line in pathlib.Path(f'/proc/{process.pid}/smaps_rollup').read_text().splitlines():
        fields = line.split()
        if len(fields) >= 2 and fields[0].startswith('Private_'):
            values[fields[0]] = int(fields[1]) * 1024
    return sum(values.values()) / 2**20


class Sample:
    def __init__(self, process):
        self.process = process
        self.done = threading.Event()
        self.memory = []
        self.samples = []
        self.errors = []

    def __enter__(self):
        self.start = time.monotonic()
        self.cpu_start = process_cpu_seconds(self.process)
        self.worker = threading.Thread(target=self.collect)
        self.worker.start()
        return self

    def collect(self):
        try:
            while not self.done.is_set():
                self.memory.append(private_memory(self.process))
                self.samples.append({'seconds': time.monotonic() - self.start,
                                     'cpu_seconds': process_cpu_seconds(self.process) - self.cpu_start,
                                     'private_resident_mib': self.memory[-1]})
                self.done.wait(1)
        except Exception as error:
            self.errors.append(str(error))

    def __exit__(self, *unused):
        self.end = time.monotonic()
        self.cpu_end = process_cpu_seconds(self.process)
        self.done.set()
        self.worker.join()
        assert not self.errors, self.errors

    def result(self):
        elapsed = self.end - self.start
        return {'measured_seconds': elapsed,
                'receiver_cpu_percent_one_core': 100 * (self.cpu_end - self.cpu_start) / elapsed,
                'private_resident_mib_median': statistics.median(self.memory),
                'private_resident_mib_peak': max(self.memory),
                'resource_samples': self.samples}


def generate(output, hls_seconds):
    output.mkdir(parents=True, exist_ok=True)
    clips = {}
    for codec, encoder, extra in (
        ('h264', 'libx264', ['-tune', 'zerolatency', '-x264-params', 'bframes=0:scenecut=0']),
        ('hevc', 'libx265', ['-x265-params', 'pools=2:frame-threads=2:bframes=0:scenecut=0:log-level=error']),
    ):
        path = output / (codec + '.mp4')
        if not path.exists():
            command('ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i',
                    'testsrc2=size=1920x1080:rate=60', '-t', '2', '-an', '-c:v', encoder,
                    '-preset', 'ultrafast', '-threads', '2', '-pix_fmt', 'yuv420p', '-g', '60',
                    '-crf', '28', *extra, path)
        probe = json.loads(command('ffprobe', '-v', 'error', '-select_streams', 'v',
                                   '-show_streams', '-show_packets', '-show_data', '-of', 'json', path))
        stream = probe['streams'][0]
        assert (stream['width'], stream['height'], stream['r_frame_rate']) == (1920, 1080, '60/1')
        assert len(probe['packets']) == 120 and 'K' in probe['packets'][0]['flags']
        config = media.ffmpeg_hex(stream['extradata'])
        if codec == 'hevc':
            box = struct.pack('>I', len(config) + 8) + b'hvcC' + config
            config = struct.pack('>I', 86 + len(box)) + b'hvc1' + bytes(78) + box
        clips[codec] = (config, [(media.ffmpeg_hex(p['data']), 'K' in p['flags']) for p in probe['packets']])
    hls = output / 'hls'
    hls.mkdir(exist_ok=True)
    # Fresh generation ensures the playlist has the requested duration.
    command('ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i',
            'testsrc2=size=1920x1080:rate=30', '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000',
            '-t', str(hls_seconds), '-c:v', 'libx264', '-preset', 'ultrafast', '-tune', 'zerolatency',
            '-threads', '2', '-crf', '28', '-pix_fmt', 'yuv420p', '-g', '30', '-c:a', 'aac',
            '-ac', '2', '-b:a', '128k', '-hls_time', '1', '-hls_list_size', '0',
            '-hls_segment_type', 'fmp4', '-hls_segment_filename', hls / 'segment%03d.m4s',
            hls / 'video.m3u8')
    manifest = {str(p.relative_to(output)): sha256(p) for p in output.rglob('*') if p.is_file()}
    return clips, hls, manifest


class Mirror:
    def __init__(self, control, key, clip):
        self.control = control
        self.clip = clip
        ident = 123456
        body = plistlib.dumps({'streams': [{'type': 110, 'streamConnectionID': ident}]}, fmt=plistlib.FMT_BINARY)
        status, _, body = control.rpc('SETUP', '/stream', body, {'Content-Type': 'application/x-apple-binary-plist'})
        assert status == 200
        port = plistlib.loads(body)['streams'][0]['dataPort']
        derive = lambda salt: hashlib.sha512((salt + str(ident)).encode() + key).digest()[:16]
        self.cipher = Cipher(algorithms.AES(derive('AirPlayStreamKey')),
                             modes.CTR(derive('AirPlayStreamIV'))).encryptor()
        self.stream = socket.create_connection(('127.0.0.1', port), timeout=10)
        self.stream.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.packet(0x100, clip[0])
        self.sent = 0
        self.lateness = []

    def packet(self, kind, payload):
        header = bytearray(128)
        struct.pack_into('<I', header, 0, len(payload))
        struct.pack_into('>H', header, 4, kind)
        now = time.time() + 2208988800
        struct.pack_into('<Q', header, 8, (int(now) << 32) | int((now % 1) * 2**32))
        self.stream.sendall(header + payload)

    def play(self, seconds):
        start = time.monotonic()
        count = round(seconds * 60)
        for index in range(count):
            due = start + index / 60
            time.sleep(max(0, due - time.monotonic()))
            self.lateness.append(max(0, time.monotonic() - due) * 1000)
            packet, keyframe = self.clip[1][self.sent % len(self.clip[1])]
            self.packet(0x10 if keyframe else 0, self.cipher.update(packet))
            self.sent += 1
        time.sleep(max(0, start + seconds - time.monotonic()))

    def close(self):
        self.stream.close()


class Audio:
    def __init__(self, connection, key):
        fixture = (media.ROOT / 'rust/tests/fixtures/eld-480.bin').read_bytes()
        packets, offset = [], 0
        while offset < len(fixture):
            size = int.from_bytes(fixture[offset:offset + 4], 'big')
            plain = fixture[offset + 4:offset + 4 + size]
            block = size // 16 * 16
            encrypted = Cipher(algorithms.AES(key), modes.CBC(bytes(16))).encryptor().update(plain[:block])
            packets.append(encrypted + plain[block:])
            offset += 4 + size
        assert packets and offset == len(fixture)
        body = plistlib.dumps({'streams': [{'type': 96, 'ct': 4, 'spf': 480}]}, fmt=plistlib.FMT_BINARY)
        status, _, reply = connection.rpc('SETUP', '/stream', body, {'Content-Type': 'application/x-apple-binary-plist'})
        assert status == 200
        self.port = plistlib.loads(reply)['streams'][0]['dataPort']
        self.packets = packets
        self.stop = threading.Event()
        self.errors = []
        self.sent = 0
        self.worker = threading.Thread(target=self.send)
        self.worker.start()

    def send(self):
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
                start = time.monotonic()
                for i in itertools.count():
                    if self.stop.wait(max(0, start + i / 100 - time.monotonic())):
                        break
                    packet = b'\x80\xe0' + struct.pack('>HI', i & 65535, (i * 480) & 0xffffffff)
                    sock.sendto(packet + bytes(4) + self.packets[i % len(self.packets)], ('127.0.0.1', self.port))
                    self.sent += 1
        except Exception as error:
            self.errors.append(str(error))

    def close(self):
        self.stop.set()
        self.worker.join(timeout=5)
        assert not self.worker.is_alive() and not self.errors, self.errors


class Hls:
    """FCUP resources traverse AirPlay /reverse and /action, including ownership gating."""
    def __init__(self, port, directory):
        self.port = port
        self.prefix = 'mlhls://benchmark/'
        self.resources = {self.prefix + p.name: p.read_bytes() for p in directory.iterdir() if p.is_file()}
        self.resources[self.prefix + 'master.m3u8'] = b'#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=8000000\nvideo.m3u8\n'
        self.headers = {'X-Apple-Session-ID': 'benchmark', 'Content-Type': 'application/x-apple-binary-plist'}
        self.reverse = media.pair.Conn('127.0.0.1', port)
        assert self.reverse.rpc('POST', '/reverse', extra_headers=self.headers)[0] == 101
        self.reverse.sock.settimeout(.2)
        self.stop = threading.Event()
        self.errors = []
        self.requests = 0
        self.worker = threading.Thread(target=self.serve)
        self.worker.start()
        self.control = media.pair.Conn('127.0.0.1', port)
        play = plistlib.dumps({'Content-Location': self.prefix + 'master.m3u8'}, fmt=plistlib.FMT_BINARY)
        assert self.control.rpc('POST', '/play', play, self.headers)[0] == 200

    def serve(self):
        buffer = self.reverse.buf
        try:
            while not self.stop.is_set():
                while b'\r\n\r\n' not in buffer:
                    try:
                        part = self.reverse.sock.recv(65536)
                    except socket.timeout:
                        if self.stop.is_set():
                            return
                        continue
                    if not part:
                        return
                    buffer += part
                head, _, buffer = buffer.partition(b'\r\n\r\n')
                length = next(int(line.split(b':', 1)[1]) for line in head.split(b'\r\n')
                              if line.lower().startswith(b'content-length:'))
                while len(buffer) < length:
                    buffer += self.reverse.sock.recv(65536)
                body, buffer = buffer[:length], buffer[length:]
                request = plistlib.loads(body)['request']
                self.reverse.sock.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n')
                url = request['FCUP_Response_URL']
                params = {**request, 'FCUP_Response_Data': self.resources[url], 'FCUP_Response_StatusCode': 200}
                action = plistlib.dumps({'type': 'unhandledURLResponse', 'params': params}, fmt=plistlib.FMT_BINARY)
                with contextlib.closing(media.pair.Conn('127.0.0.1', self.port)) as connection:
                    assert connection.rpc('POST', '/action', action, self.headers)[0] == 200
                self.requests += 1
        except Exception as error:
            if not self.stop.is_set():
                self.errors.append(str(error))

    def close(self):
        assert self.control.rpc('POST', '/stop', extra_headers=self.headers)[0] == 200
        self.stop.set()
        self.worker.join(timeout=5)
        self.reverse.close()
        self.control.close()
        assert not self.worker.is_alive() and not self.errors, self.errors


def run_case(binary, case, directory, clips, hls, args):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'settings.json').write_text(json.dumps({'vsync': False, 'hardware_decode': False,
                                                       'automatic_updates': False, 'fullscreen': False}))
    env = dict(os.environ, AIRDOCK_AUDIO_NULL='1')
    env.setdefault('RUST_LOG', 'warn')
    metrics_path = directory / 'metrics.json'
    with (directory / 'receiver.log').open('w') as log:
        process = subprocess.Popen([str(binary), '--audio-null', '--port', str(args.port),
                                    '--config-dir', str(directory), '--hls-proxy-playback',
                                    '--metrics', str(metrics_path)], stdout=log, stderr=log, env=env)
        mirror = audio = control = stream = None
        try:
            media.wait_listener(args.port, process)
            win = window(process)
            xdo('windowfocus', win)
            xdo('windowsize', win, 1000, 760)
            time.sleep(args.warmup)
            sent = 0
            if case == 'idle':
                with Sample(process) as sample:
                    time.sleep(args.idle_seconds)
            elif case == 'hls':
                stream = Hls(args.port, hls)
                time.sleep(args.warmup)
                with Sample(process) as sample:
                    time.sleep(args.seconds)
                sent = round((args.seconds + args.warmup) * 30)
            else:
                codec = 'hevc' if case == 'hevc' else 'h264'
                control = media.pair.Conn('127.0.0.1', args.port)
                key = setup_keys(control)
                audio = Audio(control, key)
                mirror = Mirror(control, key, clips[codec])
                mirror.play(args.warmup)
                if case == 'hidden':
                    xdo('windowminimize', win)
                    time.sleep(1.3)
                with Sample(process) as sample:
                    mirror.play(args.idle_seconds if case == 'hidden' else args.seconds)
                sent = mirror.sent
                if case == 'hidden':
                    visible = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(process.pid),
                                              '--name', 'AirDock'], capture_output=True, text=True)
                    assert not visible.stdout.strip(), 'Hidden playback restored unexpectedly'
            resources = sample.result()
            if mirror:
                resources['sender_lateness_p99_ms'] = sorted(mirror.lateness)[max(0, int(.99 * len(mirror.lateness)) - 1)]
            if audio:
                audio.close()
                audio = None
            if mirror:
                mirror.close()
                mirror = None
            # Keep the ownership lease until normal shutdown so queued final frames can finish.
            time.sleep(.3)
            if stream:
                resources['fcup_requests'] = stream.requests
                stream.stop.set()
                stream.worker.join(timeout=5)
            process.send_signal(signal.SIGINT)
            assert process.wait(timeout=15) == 0
        finally:
            if audio:
                audio.close()
            if mirror:
                mirror.close()
            if control:
                control.close()
            if stream:
                # Receiver has normally exited; do not send /stop to a closed listener.
                stream.stop.set()
                stream.worker.join(timeout=5)
                stream.reverse.close()
                stream.control.close()
                assert not stream.worker.is_alive() and not stream.errors, stream.errors
            if process.poll() is None:
                process.kill()
                process.wait()
    metrics = json.loads(metrics_path.read_text())
    assert metrics['audio_errors'] == 0, metrics
    if case in ('h264', 'hevc', 'hidden'):
        assert metrics['decoded_frames'] == sent, (sent, metrics)
    if case != 'idle':
        assert metrics['render_adapter'], metrics
        assert metrics['presented_frames'] > 0 and metrics['uploaded_frames'] > 0, metrics
    if case == 'hidden':
        assert metrics['presented_frames'] <= args.warmup * 60 + 60, metrics
        assert metrics['hidden_replaced_frames'] > 0, metrics
    if case == 'idle':
        assert metrics['decoded_frames'] == metrics['presented_frames'] == 0, metrics
    return {'sent_video_frames': sent, 'metrics': metrics, **resources}


def summarize(rows):
    keys = ('receiver_cpu_percent_one_core', 'private_resident_mib_median', 'private_resident_mib_peak')
    metrics = ('decoded_frames', 'presented_frames', 'replaced_frames', 'hidden_replaced_frames',
               'schedule_dropped', 'stale_dropped', 'p95_receive_to_present_us', 'p99_receive_to_present_us',
               'p95_new_submission_interval_us', 'p99_new_submission_interval_us')
    result = {}
    for case in sorted({r['case'] for r in rows}):
        result[case] = {}
        for version in ('release', 'current'):
            group = [r for r in rows if r['case'] == case and r['version'] == version]
            if not group:
                continue
            result[case][version] = {k: statistics.median(r[k] for r in group) for k in keys}
            for key in metrics:
                values = [r['metrics'][key] for r in group if r['metrics'].get(key) is not None]
                result[case][version][key] = statistics.median(values) if values else None
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--release', type=pathlib.Path, required=True)
    parser.add_argument('--current', type=pathlib.Path, required=True)
    parser.add_argument('--release-commit', required=True)
    parser.add_argument('--current-commit', required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--runs', type=int, default=5)
    parser.add_argument('--seconds', type=int, default=30)
    parser.add_argument('--idle-seconds', type=int, default=20)
    parser.add_argument('--warmup', type=int, default=3)
    parser.add_argument('--port', type=int, default=7020)
    parser.add_argument('--cases', nargs='+', choices=['h264', 'hevc', 'hls', 'idle', 'hidden'],
                        default=['h264', 'hevc', 'hls', 'idle', 'hidden'])
    args = parser.parse_args()
    assert platform.system() == 'Linux' and os.environ.get('DISPLAY'), 'Linux/X11 is required'
    assert args.runs >= 1 and args.seconds >= 1 and args.idle_seconds >= 1 and args.warmup >= 1
    binaries = {'release': args.release.resolve(), 'current': args.current.resolve()}
    args.output.mkdir(parents=True, exist_ok=True)
    clips, hls, manifest = generate(args.output / 'input', args.seconds + args.warmup + 4)
    result = {'schema': 1, 'release_commit': args.release_commit, 'current_commit': args.current_commit,
              'physical_hardware': False, 'measurement': 'Linux/X11 Vulkan submissions, software decoding, null audio',
              'limitations': ['Not official Windows binaries', 'No physical display events or display latency',
                              'No WASAPI device, audible AV offset or drift', 'No hardware GPU utilization or VRAM',
                              'Hidden X11 window is not Windows tray acceptance'],
              'cpu_definition': '100 percent equals one fully occupied core; sender excluded',
              'memory_definition': 'Linux smaps_rollup Private_Clean + Private_Dirty + Private_Hugetlb',
              'configuration': vars(args) | {'release': str(args.release), 'current': str(args.current), 'output': str(args.output)},
              'environment': {'platform': platform.platform(), 'processor': platform.processor(),
                              'cpu_quota': pathlib.Path('/sys/fs/cgroup/cpu.max').read_text().strip(),
                              'memory_limit': pathlib.Path('/sys/fs/cgroup/memory.max').read_text().strip(),
                              'xdg_data_dirs': os.environ.get('XDG_DATA_DIRS'),
                              'wgpu_backend': os.environ.get('WGPU_BACKEND'),
                              'vulkan_driver': os.environ.get('VK_DRIVER_FILES'),
                              'llvmpipe_threads': os.environ.get('LP_NUM_THREADS'),
                              'rustc': command('rustc', '--version').decode().strip(),
                              'ffmpeg': command('ffmpeg', '-version').decode()},
              'binary_sha256': {k: sha256(p) for k, p in binaries.items()},
              'input_sha256': manifest, 'runs': []}
    result['configuration']['output'] = str(args.output)
    wm = subprocess.Popen(['openbox'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(.5)
        assert wm.poll() is None, 'Openbox failed to start; check its themes and XDG_DATA_DIRS'
        for index in range(args.runs):
            order = ('release', 'current') if index % 2 == 0 else ('current', 'release')
            for case in args.cases:
                for version in order:
                    directory = args.output / f'{index}-{case}-{version}'
                    record = run_case(binaries[version], case, directory, clips, hls, args)
                    result['runs'].append({'version': version, 'case': case, 'run': index, **record})
                    result['medians'] = summarize(result['runs'])
                    (args.output / 'results.json').write_text(json.dumps(result, indent=2))
                    print(json.dumps({'case': case, 'version': version, 'run': index,
                                      'cpu_percent': record['receiver_cpu_percent_one_core'],
                                      'private_mib': record['private_resident_mib_median'],
                                      'decoded': record['metrics']['decoded_frames'],
                                      'submitted': record['metrics']['presented_frames'],
                                      'p95_us': record['metrics']['p95_receive_to_present_us'],
                                      'p99_us': record['metrics']['p99_receive_to_present_us']}), flush=True)
    finally:
        wm.terminate()
        wm.wait(timeout=5)


if __name__ == '__main__':
    main()
