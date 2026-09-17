#!/usr/bin/env python3
"""Compare JSON/MessagePack and TCP/Unix sockets using repeated HTTP samples."""

import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def positive(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def build(fmt):
    features = "client,server" + (",msgpack" if fmt == "msgpack" else "")
    command = [
        "cargo", "bench", "-p", "macroni", "--bench", "small_payloads",
        "--no-run", "--no-default-features", "--features", features,
        "--message-format=json-render-diagnostics",
    ]
    result = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, text=True)
    executable = None
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact"
                and message["target"]["name"] == "small_payloads"
                and message.get("executable")):
            executable = message["executable"]
    result.check_returncode()
    if executable is None:
        raise RuntimeError("Cargo did not report the benchmark executable")
    return executable


def report(rows, transports):
    def values(fmt, payload, kind, metric, concurrency=None, transport=None):
        return [r[metric] for r in rows if r["format"] == fmt
                and r["payload"] == payload and r["kind"] == kind
                and r.get("concurrency") == concurrency
                and (transport is None or r["transport"] == transport)]

    print("\nMedian across samples; body bytes exclude HTTP headers and framing.\n")
    print("| Payload | JSON bytes | MsgPack bytes | Size change | JSON codec ns | MsgPack codec ns |")
    print("|---|---:|---:|---:|---:|---:|")
    for payload in ("telemetry", "short_text"):
        sizes = [statistics.median(values(f, payload, "codec", "body_bytes"))
                 for f in ("json", "msgpack")]
        times = [statistics.median(values(f, payload, "codec", "encode_decode_ns"))
                 for f in ("json", "msgpack")]
        print(f"| {payload} | {sizes[0]:.1f} | {sizes[1]:.1f} | "
              f"{(sizes[1] / sizes[0] - 1) * 100:+.1f}% | {times[0]:.0f} | {times[1]:.0f} |")

    for transport in transports:
        print(f"\nHTTP over {transport.upper()}:\n")
        print("| Payload | Concurrency | JSON req/s (min–max) | MsgPack req/s (min–max) | Change |")
        print("|---|---:|---:|---:|---:|")
        for payload in ("telemetry", "short_text"):
            for concurrency in (1, 16):
                rates = [values(f, payload, "http", "requests_per_second", concurrency, transport)
                         for f in ("json", "msgpack")]
                medians = [statistics.median(v) for v in rates]
                cells = [f"{statistics.median(v):,.0f} ({min(v):,.0f}–{max(v):,.0f})" for v in rates]
                print(f"| {payload} | {concurrency} | {cells[0]} | {cells[1]} | "
                      f"{(medians[1] / medians[0] - 1) * 100:+.1f}% |")

        print("\n| Payload | Concurrency | JSON p50 / p95 µs | MsgPack p50 / p95 µs |")
        print("|---|---:|---:|---:|")
        for payload in ("telemetry", "short_text"):
            for concurrency in (1, 16):
                cells = []
                for fmt in ("json", "msgpack"):
                    p50, p95 = [statistics.median(values(fmt, payload, "http", metric, concurrency, transport))
                                for metric in ("p50_us", "p95_us")]
                    cells.append(f"{p50:.1f} / {p95:.1f}")
                print(f"| {payload} | {concurrency} | {cells[0]} | {cells[1]} |")

    if {"tcp", "uds"}.issubset(transports):
        print("\nTransport comparison (same format and payload):\n")
        print("| Format | Payload | Concurrency | TCP req/s | UDS req/s | Change |")
        print("|---|---|---:|---:|---:|---:|")
        for fmt in ("json", "msgpack"):
            for payload in ("telemetry", "short_text"):
                for concurrency in (1, 16):
                    tcp, uds = [statistics.median(values(fmt, payload, "http", "requests_per_second", concurrency, t))
                                for t in ("tcp", "uds")]
                    print(f"| {fmt} | {payload} | {concurrency} | {tcp:,.0f} | {uds:,.0f} | "
                          f"{(uds / tcp - 1) * 100:+.1f}% |")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=positive, default=5)
    parser.add_argument("--requests", type=positive, default=10_000,
                        help="total measured requests per payload/concurrency/sample")
    parser.add_argument("--codec-iterations", type=positive, default=100_000)
    parser.add_argument("--transports", nargs="+", choices=("tcp", "uds"), default=["tcp"])
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    args.transports = list(dict.fromkeys(args.transports))
    if "uds" in args.transports and os.name != "posix":
        parser.error("Unix domain socket benchmarks require a Unix-like platform")
    if args.output is None:
        filename = "bench-formats.json" if args.transports == ["tcp"] else "bench-transports.json"
        args.output = ROOT / "target" / filename
    binaries = {fmt: build(fmt) for fmt in ("json", "msgpack")}
    env = dict(os.environ, MACRONI_BENCH_REQUESTS=str(args.requests),
               MACRONI_BENCH_CODEC_ITERATIONS=str(args.codec_iterations))
    rows = []
    cases = [(fmt, transport) for transport in args.transports for fmt in ("json", "msgpack")]
    for sample in range(args.samples):
        # Rotate all combinations through the run positions, including transports.
        offset = sample % len(cases)
        order = cases[offset:] + cases[:offset]
        for fmt, transport in order:
            print(f"Sample {sample + 1}/{args.samples}: {fmt}/{transport}", file=sys.stderr, flush=True)
            result = subprocess.run([binaries[fmt]], cwd=ROOT,
                                    env=dict(env, MACRONI_BENCH_TRANSPORT=transport),
                                    stdout=subprocess.PIPE, text=True, check=True)
            measurements = [json.loads(line) for line in result.stdout.splitlines()]
            if len(measurements) != 6 or any(r["format"] != fmt or r["transport"] != transport
                                             for r in measurements):
                raise RuntimeError("unexpected benchmark output")
            rows.extend(dict(row, sample=sample + 1) for row in measurements)

    metadata = {
        "utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(),
        "logical_cpus": os.cpu_count(),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "working_tree_changes": subprocess.check_output(
            ["git", "status", "--short"], cwd=ROOT, text=True).splitlines(),
        "samples": args.samples,
        "transports": args.transports,
        "execution_order": "rotating format/transport combinations",
        "requests_per_case": args.requests,
        "codec_iterations": args.codec_iterations,
        "worker_threads": 4,
        "gzip": False,
    }
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.exists():
        metadata["cpu"] = next((line.split(":", 1)[1].strip()
                                for line in cpuinfo.read_text().splitlines()
                                if line.startswith("model name")), platform.machine())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"metadata": metadata, "measurements": rows}, indent=2) + "\n")
    report(rows, args.transports)
    print(f"\nRaw measurements and environment: {args.output}")


if __name__ == "__main__":
    main()
