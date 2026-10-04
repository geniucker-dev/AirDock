# SPDX-License-Identifier: MPL-2.0
"""Verify normal cleanup on Unix termination while an encrypted mirror session is active.
Windows console/tray shutdown remains separately exercised by platform acceptance.
"""
import argparse
import json
import os
import pathlib
import signal
import socket
import subprocess
import threading
import time
import rust_integration as media
from rust_reconnect_integration import setup_keys


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=pathlib.Path, required=True)
    args = p.parse_args()
    assert os.name == 'posix'
    output = media.ROOT / 'rust-validation' / 'shutdown'
    output.mkdir(parents=True, exist_ok=True)
    results = {}
    for termination in (signal.SIGINT, signal.SIGTERM):
        directory = output / (termination.name + '-接收器-é')
        directory.mkdir(exist_ok=True)
        paths = media.generate_media(directory)
        metrics = directory / 'metrics.json'
        metrics.unlink(missing_ok=True)
        with (directory / 'receiver.log').open('w') as log:
            process = subprocess.Popen([str(args.binary.resolve()), '--headless', '--audio-null', '--port', '7015',
                                        '--config-dir', str(directory), '--metrics', str(metrics)], stdout=log, stderr=log)
            connection = None
            sender = None
            try:
                media.wait_listener(7015, process)
                connection = media.pair.Conn('127.0.0.1', 7015)
                key = setup_keys(connection)
                def send():
                    try:
                        media.send_mirror(connection, key, paths[:1])
                    except (OSError, EOFError):
                        pass  # Cleanup intentionally closes the active data connection.
                sender = threading.Thread(target=send)
                sender.start()
                time.sleep(.35)
                started = time.monotonic()
                process.send_signal(termination)
                assert process.wait(timeout=5) == 0
                duration = time.monotonic() - started
                record = json.loads(metrics.read_text())
                assert record['decoded_frames'] > 0, record
                assert record['pending_scheduled_frames'] == 0, record
                results[termination.name] = {'cleanup_seconds': duration, 'decoded_frames_before_exit': record['decoded_frames']}
            finally:
                if connection:
                    connection.close()
                if sender:
                    sender.join(timeout=2)
                    assert not sender.is_alive()
                if process.poll() is None:
                    process.kill()
                    process.wait()
        with socket.socket() as released:
            released.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            released.bind(('127.0.0.1', 7015))
    (output / 'results.json').write_text(json.dumps(results, indent=2))
    print(json.dumps(results, indent=2))


if __name__ == '__main__':
    main()
