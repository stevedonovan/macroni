# Localhost TCP versus Unix domain sockets

Measured on 2026-09-16 with:

```sh
python3 scripts/bench-formats.py --transports tcp uds
```

Intel Core i7-1185G7 (8 logical CPUs), WSL2/Linux 6.6.87.2, Rust 1.96.0,
optimized Cargo bench builds, four Tokio worker threads. Five samples per
format/transport combination, 10,000 requests per payload/concurrency case:
800,000 measured HTTP requests in total, plus warmups. Run order rotates through
the four combinations. The working tree is based on `1f645e03`, including the
MessagePack and benchmark changes.

Both transports use HTTP/1 with warmed persistent connections. Payloads, handler,
generated client, connection-pool settings, and validation are identical. The
benchmark uses the listener helpers underlying `macroni::serve!` and macroni's
runtime address parser. It also checks each address using the normal generated
client constructor before timing. Socket files live in private temporary
directories; no socket directories remained after this run.

See the [methodology](README.md) and
[raw measurements](results/transports-2026-09-16.json).

## Throughput

Requests per second: median across five samples, with minimum–maximum in parentheses.
Change is the ratio of medians for UDS versus TCP.

| Format | Payload | Concurrency | TCP req/s | UDS req/s | Change |
|---|---|---:|---:|---:|---:|
| JSON | Telemetry | 1 | 8,472 (7,526–10,769) | 10,446 (8,443–12,417) | +23.3% |
| JSON | Telemetry | 16 | 63,683 (53,981–90,068) | 86,544 (67,478–103,606) | +35.9% |
| JSON | Short text | 1 | 7,550 (6,958–10,268) | 9,774 (7,482–12,296) | +29.4% |
| JSON | Short text | 16 | 62,002 (57,334–84,300) | 82,820 (71,095–106,763) | +33.6% |
| MessagePack | Telemetry | 1 | 8,882 (7,235–10,960) | 10,198 (8,527–12,943) | +14.8% |
| MessagePack | Telemetry | 16 | 73,568 (67,313–84,135) | 89,516 (69,115–108,815) | +21.7% |
| MessagePack | Short text | 1 | 8,683 (7,191–10,687) | 10,263 (8,811–12,617) | +18.2% |
| MessagePack | Short text | 16 | 68,486 (61,630–89,774) | 85,905 (71,907–97,740) | +25.4% |

## Latency

Medians of each sample's p50/p95 latencies, in microseconds:

| Format | Payload | Concurrency | TCP p50 / p95 | UDS p50 / p95 |
|---|---|---:|---:|---:|
| JSON | Telemetry | 1 | 112.5 / 168.1 | 91.9 / 133.1 |
| JSON | Telemetry | 16 | 238.3 / 391.9 | 171.6 / 300.7 |
| JSON | Short text | 1 | 124.5 / 187.6 | 98.9 / 151.0 |
| JSON | Short text | 16 | 240.4 / 400.8 | 181.6 / 309.1 |
| MessagePack | Telemetry | 1 | 107.3 / 154.7 | 94.4 / 137.6 |
| MessagePack | Telemetry | 16 | 202.3 / 352.2 | 167.0 / 292.1 |
| MessagePack | Short text | 1 | 109.3 / 158.1 | 93.7 / 140.6 |
| MessagePack | Short text | 16 | 219.3 / 364.2 | 175.0 / 299.3 |

UDS had higher median throughput and lower median latency in every case. The
observed median throughput differences were approximately 15–36%. However, sample
ranges overlap and this run varied substantially more than the earlier
[format-only experiment](RESULTS.md). Treat the magnitude as provisional and
repeat on a quiet machine; do not compare absolute rates across those two runs
as though they were controlled measurements of the same conditions.

The pattern is consistent with the transport affecting small-request overhead,
but this benchmark does not isolate kernel time, scheduling, or HTTP parsing.
It measures steady-state requests, not connection setup. These results are
specific to local communication on this WSL2 machine.
