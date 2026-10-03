"""Independent encrypted mirroring across session and TCP reconnects."""
import argparse
import json
import os
import pathlib
import plistlib
import socket
import struct
import subprocess
import sys
import time

import rust_integration as media


def process_cpu_seconds(process):
    if sys.platform == 'win32':
        import ctypes
        from ctypes import wintypes
        get_times = ctypes.WinDLL('kernel32', use_last_error=True).GetProcessTimes
        get_times.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
        get_times.restype = wintypes.BOOL
        times = [wintypes.FILETIME() for _ in range(4)]
        if not get_times(int(process._handle), *(ctypes.byref(t) for t in times)):
            raise ctypes.WinError(ctypes.get_last_error())
        return sum((t.dwHighDateTime << 32) | t.dwLowDateTime for t in times[2:]) / 10000000
    # Linux /proc CPU counters exclude the sender and other CI processes.
    fields = pathlib.Path(f'/proc/{process.pid}/stat').read_text().rsplit(')', 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK')


def setup_keys(connection):
    encrypted, key = media.handshake(connection)
    body = plistlib.dumps({'ekey': encrypted, 'eiv': bytes(16)}, fmt=plistlib.FMT_BINARY)
    assert connection.rpc('SETUP', '/stream', body,
                          {'Content-Type': 'application/x-apple-binary-plist'})[0] == 200
    return key


def teardown(connection, streams_only=False):
    body = plistlib.dumps({'streams': [{'type': 110}]}, fmt=plistlib.FMT_BINARY) if streams_only else b''
    assert connection.rpc('TEARDOWN', '/stream', body)[0] == 200


def broken_connection(port, record, reset=True):
    with socket.create_connection(('127.0.0.1', port), timeout=3) as stream:
        stream.sendall(record)
        # Let the receiver accept and start reading before triggering a reset.
        time.sleep(.1)
        if reset:
            layout = 'HH' if sys.platform == 'win32' else 'ii'
            stream.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack(layout, 1, 0))
    time.sleep(.2)


def wire_reconnects(port, paths, idle_check):
    checks = []
    connection = media.pair.Conn('127.0.0.1', port)
    try:
        idle_check('control connection')
        key = setup_keys(connection)
        ids = [0, 123456, (1 << 63) - 1, 1 << 63, 0xfedcba9876543210, (1 << 64) - 1]
        for index, ident in enumerate(ids):
            media.send_mirror(connection, key, [paths[index % len(paths)]], ident=ident)
            checks.append(f'reused control connection, eight-byte ID {ident}')
            streams_only = index % 2 == 0
            teardown(connection, streams_only)
            if not streams_only:
                key = setup_keys(connection)

        # A new control socket can claim a session before the old socket closes.
        # Unlike an audio-only SETUP check, require video decrypted by its new key.
        for ident in [0x8000000000000123, 0xffffffffffffffff]:
            replacement = media.pair.Conn('127.0.0.1', port)
            try:
                new_key = setup_keys(replacement)
            except BaseException:
                replacement.close()
                raise
            connection.close()
            connection = replacement
            key = new_key
            media.send_mirror(connection, key, [paths[2]], ident=ident, signed_id=False)
            checks.append(f'replacement control connection, positive wide ID {ident}')

        # Keep the negotiated port alive across clean EOF, resets in incomplete
        # headers/payloads, and a bad codec configuration. CTR restarts per TCP.
        ident = 0xfedcba9876543210
        data_port = media.send_mirror(connection, key, [paths[0]], ident=ident)
        checks.append('first connection on advertised video port')
        with socket.create_connection(('127.0.0.1', data_port), timeout=3):
            idle_check('control and video connections')
        media.send_mirror(connection, key, [paths[1]], ident=ident, port=data_port)
        checks.append('clean EOF, reconnect to same video port')
        header = bytearray(128)
        struct.pack_into('<I', header, 0, 64)
        struct.pack_into('>H', header, 4, 0x100)
        malformed = bytearray(header)
        struct.pack_into('<I', malformed, 0, 4)
        for label, record, reset in [
            ('TCP reset during header', bytes(37), True),
            ('TCP reset during payload', header + b'xx', True),
            ('malformed codec configuration', malformed + b'bad!', False),
        ]:
            broken_connection(data_port, record, reset)
            media.send_mirror(connection, key, [paths[2]], ident=ident, port=data_port)
            checks.append(label + ', reconnect to same video port')
        teardown(connection)
    finally:
        connection.close()
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    parser.add_argument('--gui', action='store_true')
    args = parser.parse_args()
    output = media.ROOT / 'rust-validation' / ('reconnect-gui' if args.gui else 'reconnect')
    output.mkdir(parents=True, exist_ok=True)
    config = output / 'config'
    config.mkdir(exist_ok=True)
    # Software decoding makes the exact frame count portable across CI runners.
    (config / 'settings.json').write_text(json.dumps({'hardware_decode': False, 'vsync': False}))
    paths = media.generate_media(output)
    env = os.environ.copy()
    env['SDL_AUDIODRIVER'] = 'dummy'
    port = 7013
    idle_cpu = {}
    with (output / 'receiver.log').open('w') as log:
        process = subprocess.Popen([
            str(args.binary.resolve()), *([] if args.gui else ['--headless']),
            '--port', str(port), '--config-dir', str(config),
            '--metrics', str(output / 'metrics.json'), '--exit-after', '35',
        ], env=env, stdout=log, stderr=log)
        def idle_check(label):
            time.sleep(.1)  # Allow accept to finish before sampling.
            before = process_cpu_seconds(process)
            time.sleep(1)
            used = process_cpu_seconds(process) - before
            idle_cpu[label] = used
            # No frames/requests are sent during this interval. Busy-polling one
            # idle socket consumes about a full core, not these bounded wakeups.
            if not args.gui:
                assert used < .35, (label, 'idle CPU seconds over one second', used)
        try:
            media.wait_listener(port, process)
            checks = wire_reconnects(port, paths, idle_check)
        finally:
            try:
                process.wait(timeout=40)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise
    assert process.returncode == 0, process.returncode
    metrics = json.loads((output / 'metrics.json').read_text())
    assert metrics['decoded_frames'] == len(checks) * 30, (checks, metrics)
    if args.gui:
        assert metrics['presented_frames'] > 0 and metrics['p95_receive_to_present_us'] > 0, metrics
    result = {'reconnect_checks': checks, 'idle_cpu_seconds': idle_cpu,
              'metrics': metrics, 'hardware_parity': 'pending'}
    (output / 'results.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
