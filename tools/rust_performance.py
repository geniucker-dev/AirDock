"""Validate same-hardware measured C++, Slint and Iced acceptance data.
Never infer display frames or AV synchronization from renderer submissions.
"""
import argparse
import json
import pathlib
import statistics

VERSIONS = ('cpp', 'slint', 'iced')
METRICS = ('new_displayed_fps', 'dropped_source_frames', 'p95_frame_interval_ms',
           'p99_frame_interval_ms', 'p95_processing_ms', 'av_offset_ms',
           'av_drift_ms_per_30min', 'cpu_percent', 'gpu_percent', 'private_memory_mb',
           'dedicated_gpu_memory_mb', 'shared_gpu_memory_mb')


def evaluate(data):
    assert data.get('physical_hardware') is True, 'Physical hardware evidence required'
    assert data.get('display_measurement') not in (None, 'present_return', 'gpu_submission'), 'Independent display measurement required'
    assert data.get('av_measurement'), 'Observed AV measurement method required'
    assert data.get('binary_sha256', {}).keys() >= set(VERSIONS), 'Three exact binaries required'
    assert data.get('device') and data.get('input_sha256') and data.get('power_mode'), 'Same-device/input evidence required'
    tolerance = data['limits']
    results = []
    for case in data['cases']:
        assert case['state'] in ('playing', 'idle', 'tray')
        assert case['refresh_hz'] > 0
        rows = case['runs']
        medians = {}
        for version in VERSIONS:
            samples = [r for r in rows if r['version'] == version]
            assert len(samples) >= 5, f'{case["name"]}: five runs required for {version}'
            for sample in samples:
                assert sample['duration_seconds'] >= (1800 if case['state'] == 'playing' else 60)
                assert all(m in sample and isinstance(sample[m], (int, float)) for m in METRICS)
                assert all(sample[m] >= 0 for m in METRICS if m not in ('av_offset_ms', 'av_drift_ms_per_30min'))
            medians[version] = {m: statistics.median(s[m] for s in samples) for m in METRICS}
        failures = []
        current = medians['iced']
        for reference in ('cpp', 'slint'):
            base = medians[reference]
            if case['state'] == 'playing':
                if current['new_displayed_fps'] < base['new_displayed_fps'] * .95:
                    failures.append(f'{reference}: new-frame rate regression')
                for metric in ('p95_frame_interval_ms', 'p99_frame_interval_ms', 'p95_processing_ms'):
                    if current[metric] > base[metric] + 1000 / case['refresh_hz']:
                        failures.append(f'{reference}: {metric} regression')
                if current['dropped_source_frames'] > base['dropped_source_frames'] + tolerance['extra_dropped_frames']:
                    failures.append(f'{reference}: dropped source frames')
            for metric in ('cpu_percent', 'gpu_percent', 'private_memory_mb', 'dedicated_gpu_memory_mb', 'shared_gpu_memory_mb'):
                # Absolute tolerance is necessary for counters close to zero.
                if current[metric] > base[metric] * 1.1 + tolerance['resource_absolute'][metric]:
                    failures.append(f'{reference}: {metric} regression')
        if case['state'] == 'playing':
            if abs(current['av_offset_ms']) > tolerance['av_offset_ms']:
                failures.append('AV offset exceeds predeclared tolerance')
            if abs(current['av_drift_ms_per_30min']) > tolerance['av_drift_ms_per_30min']:
                failures.append('Long-term AV drift exceeds predeclared tolerance')
        results.append({'name': case['name'], 'state': case['state'], 'medians': medians, 'failures': failures, 'pass': not failures})
    assert {c['state'] for c in data['cases']} == {'playing', 'idle', 'tray'}, 'All lifecycle states required'
    return {'hardware_performance_acceptance': 'passed' if all(r['pass'] for r in results) else 'failed', 'cases': results}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--measurements', type=pathlib.Path)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    if args.measurements:
        result = evaluate(json.loads(args.measurements.read_text()))
    else:
        result = {'hardware_performance_acceptance': 'pending',
                  'reason': 'No physical same-device comparison measurements supplied',
                  'versions': list(VERSIONS), 'required_metrics': list(METRICS)}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))
    if result['hardware_performance_acceptance'] == 'failed':
        raise SystemExit(1)


if __name__ == '__main__':
    main()
