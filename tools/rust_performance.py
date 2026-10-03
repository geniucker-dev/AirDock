"""Paired CPU conversion check. This does not measure AirPlay presentation FPS."""
import argparse
import json
import pathlib
import statistics
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference', type=pathlib.Path, required=True)
    parser.add_argument('--rust', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--runs', type=int, default=5)
    parser.add_argument('--frames', type=int, default=600)
    args = parser.parse_args()
    if args.runs < 5 or args.frames < 60:
        parser.error('At least five paired runs and 60 measured frames are required')
    cases = []
    for width, height in [(1920, 1080), (2560, 1440), (3840, 2160)]:
        rows = []
        for index in range(args.runs):
            row = {}
            order = [('reference', args.reference), ('rust', args.rust)]
            if index % 2:
                order.reverse()
            for name, binary in order:
                row[name] = json.loads(subprocess.check_output([str(binary.resolve()), str(width), str(height), str(args.frames)]))
            rows.append(row)
        ratios = [r['rust']['conversion_frames_per_second'] / r['reference']['conversion_frames_per_second'] for r in rows]
        ratio = statistics.median(ratios)
        cases.append({'width': width, 'height': height, 'paired_median_ratio': ratio, 'pass': ratio >= .95, 'runs': rows})
    result = {'measurement': 'CPU frame reference + NV12 full-range BT.709 to BGRA conversion',
              'hardware_airplay_parity': 'pending', 'cases': cases}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items() if k != 'cases'}))
    for case in cases:
        print(f"{case['width']}x{case['height']}: Rust / C++ median {case['paired_median_ratio']:.3f}, {'PASS' if case['pass'] else 'FAIL'}")
    if not all(c['pass'] for c in cases):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
