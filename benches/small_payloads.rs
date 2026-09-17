//! Run directly with `cargo bench --bench small_payloads`, or compare formats
//! using `python3 scripts/bench-formats.py`. Output is JSON Lines.

use macroni::{Result, api};
use serde::{Deserialize, Serialize};
use std::{hint::black_box, sync::Arc, time::Instant};
use tokio::sync::Barrier;

struct SocketDirectory(std::path::PathBuf);

impl SocketDirectory {
    fn new() -> Self {
        use std::os::unix::fs::DirBuilderExt;
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("macroni-{}-{unique:x}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }

    fn socket(&self) -> std::path::PathBuf {
        self.0.join("api.sock")
    }
}

impl Drop for SocketDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.socket());
        let _ = std::fs::remove_dir(&self.0);
    }
}

struct LocalServer {
    address: String,
    task: tokio::task::JoinHandle<()>,
    _socket: Option<SocketDirectory>,
}

impl LocalServer {
    async fn start(transport: &str) -> Self {
        let router = BenchmarkApiServer::router(Echo);
        // Use the same listener helpers as serve!, binding before clients start.
        match transport {
            "tcp" => {
                let listener = macroni::tcp_listener("127.0.0.1:0").await.unwrap();
                Self {
                    address: format!("http://{}", listener.local_addr().unwrap()),
                    task: tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
                    _socket: None,
                }
            }
            "uds" => {
                let directory = SocketDirectory::new();
                let address = directory.socket().to_str().unwrap().to_owned();
                let listener = macroni::unix_listener(&address).await.unwrap();
                Self {
                    address,
                    task: tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
                    _socket: Some(directory),
                }
            }
            _ => panic!("MACRONI_BENCH_TRANSPORT must be tcp or uds"),
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

const CORPUS_SIZE: usize = 256;
const WORKER_THREADS: usize = 4;
const WARMUP_PER_CLIENT: usize = 128;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    id: u32,
    device: String,
    reading: f64,
    active: bool,
    note: String,
}

#[api(client, server)]
pub trait BenchmarkApi {
    #[post("/event")]
    #[body(event)]
    async fn exchange(&self, event: Event) -> Result<Event>;
}

struct Echo;

impl BenchmarkApi for Echo {
    async fn exchange(&self, event: Event) -> Result<Event> {
        Ok(event)
    }
}

fn encode(event: &Event) -> Vec<u8> {
    #[cfg(feature = "msgpack")]
    return rmp_serde::to_vec_named(event).unwrap();
    #[cfg(not(feature = "msgpack"))]
    return serde_json::to_vec(event).unwrap();
}

fn decode(bytes: &[u8]) -> Event {
    #[cfg(feature = "msgpack")]
    return rmp_serde::from_slice(bytes).unwrap();
    #[cfg(not(feature = "msgpack"))]
    return serde_json::from_slice(bytes).unwrap();
}

fn corpus(with_text: bool) -> Vec<Event> {
    (0..CORPUS_SIZE)
        .map(|i| Event {
            id: i as u32 + 1,
            device: format!("sensor-{:02}", i % 32),
            reading: 18.25 + (i % 100) as f64 / 4.0,
            active: i % 3 != 0,
            note: if with_text {
                format!(
                    "Sample {i}: temperature is within the expected range. \
                     The device reported normally during the last collection interval; \
                     no maintenance is currently required."
                )
            } else {
                String::new()
            },
        })
        .collect()
}

fn codec_sample(events: &[Event], iterations: usize) -> f64 {
    for event in events {
        assert_eq!(decode(&encode(event)), *event);
    }
    for i in 0..10_000 {
        let bytes = encode(black_box(&events[i % events.len()]));
        black_box(decode(black_box(&bytes)));
    }
    let start = Instant::now();
    for i in 0..iterations {
        let bytes = encode(black_box(&events[i % events.len()]));
        black_box(decode(black_box(&bytes)));
    }
    start.elapsed().as_secs_f64() * 1e9 / iterations as f64
}

fn positive_env(name: &str, default: usize) -> usize {
    let value = std::env::var(name)
        .map(|s| {
            s.parse::<usize>()
                .expect("benchmark settings must be integers")
        })
        .unwrap_or(default);
    assert!(value > 0, "{name} must be positive");
    value
}

fn percentile(sorted: &[u64], percent: usize) -> f64 {
    let index = (sorted.len() * percent).div_ceil(100) - 1;
    sorted[index] as f64 / 1000.0
}

async fn http_sample(
    client: BenchmarkApiClient,
    events: Arc<Vec<Event>>,
    requests: usize,
    concurrency: usize,
) -> (f64, f64, f64) {
    let mut warmups = Vec::new();
    for worker in 0..concurrency {
        let client = client.clone();
        let events = events.clone();
        warmups.push(tokio::spawn(async move {
            for i in 0..WARMUP_PER_CLIENT {
                let event = &events[(worker + i) % events.len()];
                assert_eq!(client.exchange(event.clone()).await.unwrap(), *event);
            }
        }));
    }
    for warmup in warmups {
        warmup.await.expect("benchmark warmup");
    }
    let ready = Arc::new(Barrier::new(concurrency + 1));
    let start_gate = Arc::new(Barrier::new(concurrency + 1));
    let mut workers = Vec::new();
    for worker in 0..concurrency {
        let client = client.clone();
        let events = events.clone();
        let ready = ready.clone();
        let start_gate = start_gate.clone();
        workers.push(tokio::spawn(async move {
            let count = requests / concurrency + usize::from(worker < requests % concurrency);
            let mut latencies = Vec::with_capacity(count);
            ready.wait().await;
            start_gate.wait().await;
            for i in 0..count {
                let event = &events[(worker + i * concurrency) % events.len()];
                let start = Instant::now();
                let response = client.exchange(event.clone()).await.unwrap();
                latencies.push(start.elapsed().as_nanos() as u64);
                assert_eq!(response, *event);
            }
            latencies
        }));
    }
    ready.wait().await;
    let start = Instant::now();
    start_gate.wait().await;
    let mut latencies = Vec::with_capacity(requests);
    for worker in workers {
        latencies.extend(worker.await.expect("benchmark worker"));
    }
    let seconds = start.elapsed().as_secs_f64();
    assert_eq!(latencies.len(), requests);
    latencies.sort_unstable();
    (
        requests as f64 / seconds,
        percentile(&latencies, 50),
        percentile(&latencies, 95),
    )
}

fn main() {
    assert!(
        !cfg!(feature = "gzip"),
        "disable gzip to isolate the wire format"
    );
    let requests = positive_env("MACRONI_BENCH_REQUESTS", 10_000);
    let transport = std::env::var("MACRONI_BENCH_TRANSPORT").unwrap_or_else(|_| "tcp".into());
    assert!(
        matches!(transport.as_str(), "tcp" | "uds"),
        "unknown transport"
    );
    let codec_iterations = positive_env("MACRONI_BENCH_CODEC_ITERATIONS", 100_000);
    let format = if cfg!(feature = "msgpack") {
        "msgpack"
    } else {
        "json"
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKER_THREADS)
        .enable_all()
        .build()
        .unwrap();
    for (name, with_text) in [("telemetry", false), ("short_text", true)] {
        let events = Arc::new(corpus(with_text));
        let mean_bytes = events
            .iter()
            .map(|event| encode(event).len())
            .sum::<usize>() as f64
            / events.len() as f64;
        let codec_ns = codec_sample(&events, codec_iterations);
        println!(
            "{}",
            serde_json::json!({
                "format": format, "transport": transport, "payload": name, "kind": "codec",
                "iterations": codec_iterations, "body_bytes": mean_bytes,
                "encode_decode_ns": codec_ns,
            })
        );
        runtime.block_on(async {
            let mut server = LocalServer::start(&transport).await;
            // Verify the public constructor's automatic socket-path handling too.
            let ordinary_client = BenchmarkApiClient::new(&server.address).unwrap();
            assert_eq!(
                ordinary_client.exchange(events[0].clone()).await.unwrap(),
                events[0]
            );
            drop(ordinary_client);
            for concurrency in [1, 16] {
                // A fresh pool per case; warmup establishes reusable HTTP/1 connections.
                let (base_url, socket) = macroni::__private::parse_url(&server.address).unwrap();
                let mut http = reqwest::Client::builder()
                    .no_proxy()
                    .no_gzip()
                    .http1_only()
                    .pool_max_idle_per_host(concurrency)
                    .timeout(std::time::Duration::from_secs(10));
                if let Some(socket) = socket {
                    http = http.unix_socket(socket);
                }
                let client =
                    BenchmarkApiClient::with_http_client(base_url, http.build().unwrap()).unwrap();
                let (rps, p50, p95) =
                    http_sample(client, events.clone(), requests, concurrency).await;
                println!(
                    "{}",
                    serde_json::json!({
                        "format": format, "transport": transport, "payload": name, "kind": "http",
                        "requests": requests, "concurrency": concurrency,
                        "worker_threads": WORKER_THREADS, "warmup_per_client": WARMUP_PER_CLIENT,
                        "requests_per_second": rps, "p50_us": p50, "p95_us": p95,
                    })
                );
            }
            server.task.abort();
            let _ = (&mut server.task).await;
        });
    }
}
