# Small-payload format and transport benchmark

An [initial measured comparison](RESULTS.md) is included with raw samples.
There is also a [TCP/UDS comparison](TRANSPORT_RESULTS.md) with both formats.

From the repository root:

```sh
python3 scripts/bench-formats.py
```

The script builds optimized JSON and MessagePack versions of `small_payloads`
before measuring anything, then alternates their execution order over five
samples. It prints median results and throughput ranges, and saves every sample
and environment information to `target/bench-formats.json`. It needs Python 3,
Cargo, and permission to bind a loopback TCP listener. No Python packages or
benchmark framework are required.

For a longer run:

```sh
python3 scripts/bench-formats.py --samples 10 --requests 50000 --codec-iterations 1000000
```

`--requests` is the total measured HTTP request count per payload/concurrency case
in each sample, divided across the concurrent clients. The default is 400,000
measured requests across both formats and all cases, plus warmup requests.
`--codec-iterations` controls the separate serialization measurement. Use
`--output path.json` to preserve results elsewhere.

## TCP versus Unix domain sockets

```sh
python3 scripts/bench-formats.py --transports tcp uds
```

This compares both transports for both formats, using the same payloads and
concurrency levels. The default five samples perform 800,000 measured HTTP
requests, plus warmups. Each sample rotates the order of the four format/transport
combinations. Both optimized binaries are built before sampling. The report
includes TCP-versus-UDS throughput comparisons within each format, plus separate
format and latency tables for each transport. Raw results go to
`target/bench-transports.json`, leaving the default TCP-only result file intact.
Codec timings are pooled across the transport invocations; they do not use sockets.

Use `--transports uds` for a UDS-only format comparison. Unix socket measurements
require a Unix-like platform. Each server gets a private, uniquely named temporary
directory, and its socket and directory are removed when the case finishes.

The benchmark binds through `macroni::tcp_listener` or `macroni::unix_listener`,
the same helpers used by `macroni::serve!`. Binding explicitly lets it obtain an
ephemeral TCP port and ensures the server is ready before starting clients.
It checks the ordinary generated client's automatic handling of the URL or socket
path before timing. Measured clients use the same runtime address parser and
identical Reqwest settings, with `.unix_socket(path)` as the transport-specific
setting. All requests still use HTTP/1, and both transports reuse warmed connections;
this measures steady-state requests rather than connection establishment.

## Workload

Both formats process exactly the same deterministic corpus of 256 records:

- `telemetry`: ID, device name, numeric reading, boolean, and an empty note.
- `short_text`: the same fields with a short status message.

Each request sends one record to a generated `POST /event` handler, which echoes
it back unchanged. The shared trait uses `#[body(event)]`, so there is no extra
parameter wrapper. MessagePack uses named-field maps, matching macroni's actual
encoding. Request and response bodies have the same size. Reported bytes are the
mean serialized body size across the corpus, excluding headers and TCP framing.

The two measurements serve different purposes:

- **Codec:** one encode/decode pair, including allocations, using `serde_json`
  or `rmp-serde`. It runs synchronously after 10,000 warmup iterations; inputs and
  results pass through `black_box`. Each record's round trip is verified before
  timing. This isolates the format's serialization cost.
- **HTTP:** the generated Reqwest client talks to the generated Axum router over
  HTTP/1 with persistent TCP loopback or Unix socket connections. Concurrency 1 and 16 are measured
  separately on a four-thread Tokio runtime. Each client warms up with 128
  requests before a synchronized start. All replies are checked, and failed
  requests abort the benchmark. There is no TLS, proxy, gzip, database, or
  application work beyond echoing the record.

HTTP throughput includes request serialization, extraction, response serialization,
client decoding, allocation, scheduling, result validation, and latency recording.
Latency is measured per request from before input cloning until the decoded reply
arrives. p50/p95 are nearest-rank percentiles within a sample; the comparison table
reports medians of those per-sample percentiles. Sorting and startup are outside
the timed interval.

This is a closed-loop benchmark: each concurrent client starts its next request
after receiving the previous reply. Its percentiles describe this workload, not
latency under an independently imposed arrival rate. Client and server share the
same process and CPU resources. Loopback overhead can dominate very small bodies,
so a codec improvement need not translate into the same HTTP improvement.

Use a quiet machine, inspect the sample ranges, and repeat before interpreting
small differences. These measurements do not predict performance across a real
network or with application/database work. Compression is deliberately disabled
to isolate the encoding change.

## Running a single format

```sh
cargo bench -p macroni --bench small_payloads --no-default-features --features client,server
cargo bench -p macroni --bench small_payloads --no-default-features --features client,server,msgpack
```

These emit JSON Lines for one sample. Configure them with
`MACRONI_BENCH_REQUESTS` and `MACRONI_BENCH_CODEC_ITERATIONS`. The benchmark rejects
builds with `gzip` enabled to avoid accidentally comparing different workloads.
Set `MACRONI_BENCH_TRANSPORT=uds` to use Unix sockets (the default is `tcp`):

```sh
MACRONI_BENCH_TRANSPORT=uds cargo bench -p macroni --bench small_payloads --no-default-features --features client,server
```
