# Initial small-payload comparison

Measured on 2026-09-16 using `python3 scripts/bench-formats.py`:

- Intel Core i7-1185G7, 8 logical CPUs exposed to WSL2, Linux 6.6.87.2.
- Rust 1.96.0, optimized Cargo bench profile, four Tokio worker threads.
- Five samples per format, alternating JSON/MessagePack execution order.
- 10,000 requests per payload/concurrency/sample: 400,000 measured HTTP requests.
- 100,000 encode/decode iterations per payload/sample.
- Loopback HTTP/1, warmed persistent connections, gzip disabled.
- Working tree based on `1f645e03`, including the MessagePack and benchmark changes.

These are observations from one machine, not performance guarantees. See the
[methodology](README.md) and [raw measurements](results/small-payloads-2026-09-16.json).

## Body size and serialization

Body sizes are corpus means; serialization timings are sample medians for one
encode/decode pair, including allocations. Request and response bodies are identical.

| Payload | JSON bytes | MessagePack bytes | Size reduction | JSON codec ns | MessagePack codec ns |
|---|---:|---:|---:|---:|---:|
| Telemetry | 70.4 | 53.5 | 24.0% | 315 | 303 |
| Short text | 227.0 | 211.1 | 7.0% | 525 | 318 |

## HTTP throughput

Requests/second are medians, with the minimum and maximum samples in parentheses.

| Payload | Concurrency | JSON req/s | MessagePack req/s | Median change |
|---|---:|---:|---:|---:|
| Telemetry | 1 | 10,599 (10,427–10,718) | 10,873 (10,240–10,962) | +2.6% |
| Telemetry | 16 | 90,786 (72,057–94,647) | 92,984 (91,661–95,489) | +2.4% |
| Short text | 1 | 10,509 (10,233–10,635) | 10,684 (10,040–10,805) | +1.7% |
| Short text | 16 | 87,288 (84,756–93,263) | 90,505 (89,132–92,223) | +3.7% |

## HTTP latency

Medians of per-sample p50 and p95 request latencies, in microseconds:

| Payload | Concurrency | JSON p50 / p95 | MessagePack p50 / p95 |
|---|---:|---:|---:|
| Telemetry | 1 | 86.6 / 132.5 | 85.3 / 128.5 |
| Telemetry | 16 | 165.5 / 284.2 | 159.3 / 274.3 |
| Short text | 1 | 87.8 / 133.7 | 86.5 / 130.9 |
| Short text | 16 | 172.3 / 291.4 | 164.3 / 282.1 |

MessagePack consistently produced smaller bodies in this workload. The short-text
codec measurement took approximately 39% less time, but HTTP throughput medians
improved by only 1.7–3.7%. Every HTTP comparison has overlapping sample ranges;
this run does not establish a statistically reliable throughput improvement.
The much larger HTTP latency relative to the codec cost is consistent with HTTP,
scheduling, and other per-request overhead dominating these small payloads.

These results use named-field MessagePack maps, not positional arrays. Real
network bandwidth, payload shapes, compression, and application work can change
the tradeoff. Run the benchmark on the intended deployment hardware before using
these numbers to choose a format.
