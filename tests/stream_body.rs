#![cfg(all(feature = "client", feature = "server"))]
use futures_util::stream;
use macroni::{EventStream, Result, StreamBodyConfig, StreamExt};
use std::time::Duration;

#[macroni::api]
pub trait Upload {
    #[post("/upload/{name}")]
    #[stream_body]
    async fn upload(&self, name: String, items: EventStream<String>) -> Result<Vec<String>>;
}
struct Service;
impl Upload for Service {
    async fn upload(&self, name: String, mut items: EventStream<String>) -> Result<Vec<String>> {
        let slow_reply = name == "slow_reply";
        let mut result = vec![name];
        while let Some(item) = items.next().await {
            match item {
                Ok(value) => result.push(value),
                Err(error) if error.code() == Some("temporary") => result.push("error".into()),
                Err(error) => return Err(error),
            }
        }
        if slow_reply {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Ok(result)
    }
}
async fn start(config: StreamBodyConfig) -> (String, tokio::task::JoinHandle<()>) {
    let listener = macroni::tcp_listener("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let router = UploadServer::router(Service).layer(axum::Extension(config));
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (address, task)
}

#[tokio::test]
async fn generated_client_streams_data_errors_and_heartbeats() {
    let (address, task) =
        start(StreamBodyConfig::new().idle_timeout(Duration::from_millis(100))).await;
    let client = UploadClient::builder(&address)
        .unwrap()
        .timeout(Duration::from_millis(20)) // ordinary deadline must not apply
        .stream_body(StreamBodyConfig::new().heartbeat_interval(Duration::from_millis(15)))
        .build()
        .unwrap();
    let source = stream::iter([
        Ok("first 日本語\n".into()),
        Err(macroni::Error::bad_request("temporary", "try again")),
        Ok("last".into()),
    ])
    .then(|item| async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        item
    });
    let result = client
        .upload("batch".into(), EventStream::new(source))
        .await
        .unwrap();
    assert_eq!(result, ["batch", "first 日本語\n", "error", "last"]);
    task.abort();
}

#[tokio::test]
async fn raw_fragmented_ndjson_and_protocol_errors() {
    let (address, task) = start(StreamBodyConfig::new().max_record_bytes(100)).await;
    let client = reqwest::Client::new();
    let bytes = "{\"type\":\"heartbeat\"}\r\n{\"type\":\"data\",\"data\":\"日本語\"}\n"
        .as_bytes()
        .to_vec();
    let chunks = stream::iter(bytes.into_iter().map(|b| Ok::<_, std::io::Error>(vec![b])));
    let response = client
        .post(format!("{address}/upload/raw"))
        .header("content-type", "application/x-ndjson")
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    for body in [
        "{\"type\":\"data\",\"data\":\"unfinished\"}",
        "{\"type\":\"unknown\"}\n",
        "{\"type\":\"heartbeat\",\"extra\":1}\n",
        "{\"type\":\"data\"}\n",
        "not json\n",
    ] {
        let response = client
            .post(format!("{address}/upload/raw"))
            .header("content-type", "application/x-ndjson")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{body}");
    }
    let response = client
        .post(format!("{address}/upload/raw"))
        .header("content-type", "application/x-ndjson")
        .body("x".repeat(101))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    let response = client
        .post(format!("{address}/upload/raw"))
        .body("{}\n")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 415);
    task.abort();
}

#[tokio::test]
async fn idle_and_overall_deadlines_bound_uploads() {
    let (address, task) =
        start(StreamBodyConfig::new().idle_timeout(Duration::from_millis(30))).await;
    let client = UploadClient::new(&address).unwrap();
    assert!(
        client
            .upload("idle".into(), EventStream::new(stream::pending()))
            .await
            .is_err()
    );
    task.abort();

    let (address, task) = start(StreamBodyConfig::new()).await;
    let client = UploadClient::builder(&address)
        .unwrap()
        .stream_body(StreamBodyConfig::new().overall_timeout(Duration::from_millis(30)))
        .build()
        .unwrap();
    assert!(
        client
            .upload("deadline".into(), EventStream::new(stream::pending()))
            .await
            .unwrap_err()
            .is_timeout()
    );
    task.abort();
}

#[tokio::test]
async fn oversized_source_record_aborts_instead_of_truncating() {
    let (address, task) = start(StreamBodyConfig::new()).await;
    let client = UploadClient::builder(&address)
        .unwrap()
        .stream_body(StreamBodyConfig::new().max_record_bytes(64))
        .build()
        .unwrap();
    let result = client
        .upload(
            "big".into(),
            EventStream::new(stream::iter([Ok("x".repeat(100))])),
        )
        .await;
    assert!(result.is_err());
    task.abort();
}

#[tokio::test]
async fn response_deadline_starts_after_upload_and_server_deadline_bounds_heartbeats() {
    let (address, task) = start(StreamBodyConfig::new()).await;
    let client = UploadClient::builder(&address)
        .unwrap()
        .stream_timeout(macroni::StreamTimeout::new().response_headers(Duration::from_millis(30)))
        .build()
        .unwrap();
    let result = client
        .upload("slow_reply".into(), EventStream::new(stream::empty()))
        .await;
    assert!(result.unwrap_err().is_timeout());
    task.abort();

    let (address, task) = start(
        StreamBodyConfig::new()
            .idle_timeout(Duration::from_secs(1))
            .overall_timeout(Duration::from_millis(80)),
    )
    .await;
    let client = UploadClient::builder(&address)
        .unwrap()
        .stream_body(StreamBodyConfig::new().heartbeat_interval(Duration::from_millis(10)))
        .build()
        .unwrap();
    let result = client
        .upload("deadline".into(), EventStream::new(stream::pending()))
        .await;
    assert_eq!(
        result.unwrap_err().status(),
        Some(macroni::StatusCode::REQUEST_TIMEOUT)
    );
    task.abort();
}
