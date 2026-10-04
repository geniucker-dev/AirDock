# SPDX-License-Identifier: MPL-2.0
"""Verify the four cipher modes against fixed known-answer vectors."""
import argparse
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rust', type=Path, required=True)
    args = parser.parse_args()
    rows = [row.split() for row in (ROOT/'rust/tests/fixtures/crypto-vectors.txt').read_text().splitlines()]
    assert len(rows) == 256 and all(len(row) == 3 for row in rows)
    modes = {mode: 0 for mode in range(4)}
    for message, key, expected in rows:
        assert len(bytes.fromhex(message)) == 164 and len(bytes.fromhex(key)) == 72
        assert len(bytes.fromhex(expected)) == 16
        modes[bytes.fromhex(message)[12]] += 1
    assert all(count == 64 for count in modes.values())
    request = ''.join(f'{message} {key}\n' for message, key, _ in rows)
    actual = subprocess.check_output([str(args.rust.resolve())], input=request, text=True).splitlines()
    assert actual == [expected for _, _, expected in rows], 'Cipher known-answer regression'
    output = ROOT/'rust-validation'
    output.mkdir(exist_ok=True)
    report = {'vectors': 256, 'modes': modes, 'known_answers': 'passed'}
    (output/'crypto-vectors.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report))


if __name__ == '__main__':
    main()
