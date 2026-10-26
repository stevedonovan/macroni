#![cfg(all(feature = "client", feature = "server"))]

use axum::{body::Body, response::Response, routing::get};
use futures_util::{StreamExt, stream};
use macroni::{EventStream, Result, StreamRetry, StreamTimeout};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[macroni::api(client)]
pub trait Feed {
    #[get("/feed/{topic}")]
    #[sse]
    async fn subscribe(&self, topic: String, filter: String) -> Result<EventStream<String>>;
}

#[derive(Clone, Copy)]
enum Mode {
    Idle,
    Good,
    End,
    BadJson,
    Denied,
    HeadersTimeout,
    Disconnect,
    Application,
}

struct Server {
    address: String,
    requests: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn server(modes: Vec<Mode>) -> Server {
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let router = axum::Router::new().route("/feed/{topic}", get(move |request: axum::extract::Request| {
        let count = count.clone();
        let modes = modes.clone();
        async move {
            assert_eq!(request.uri().path(), "/feed/a");
            assert_eq!(request.uri().query(), Some("filter=b"));
            assert_eq!(request.headers()["authorization"], "Bearer test");
            assert_eq!(request.headers()["accept"], "text/event-stream");
            assert!(!request.headers().contains_key("last-event-id"));
            let n = count.fetch_add(1, Ordering::SeqCst);
            let mode = modes[n.min(modes.len() - 1)];
            if matches!(mode, Mode::HeadersTimeout) {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let body = match mode {
                Mode::Idle => Body::from_stream(stream::iter([Ok::<_, std::io::Error>(": open\n\n")])
                    .chain(stream::pending())),
                Mode::Disconnect => Body::from_stream(stream::iter([Ok::<_, std::io::Error>(": open\n\n")])
                    .chain(stream::once(async {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        Err(std::io::Error::other("connection lost"))
                    }))),
                Mode::Good => Body::from("id: 123\ndata: \"recovered\"\n\n"),
                Mode::BadJson => Body::from("data: malformed\n\n"),
                Mode::Application => Body::from("event: macroni-error\ndata: {\"status\":503,\"response\":{\"code\":\"busy\",\"message\":\"try later\"}}\n\ndata: \"recovered\"\n\n"),
                _ => Body::empty(),
            };
            Response::builder()
                .status(if matches!(mode, Mode::Denied) { 401 } else { 200 })
                .header("content-type", "text/event-stream")
                .body(body).unwrap()
        }
    }));
    let listener = macroni::tcp_listener("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server {
        address,
        requests,
        task,
    }
}

fn client(server: &Server, retries: u32) -> FeedClient {
    FeedClient::builder(&server.address)
        .unwrap()
        .bearer_token("test")
        .unwrap()
        .stream_timeout(
            StreamTimeout::new()
                .response_headers(Duration::from_millis(100))
                .idle_timeout(Duration::from_millis(40)),
        )
        .stream_retry(
            StreamRetry::new()
                .max_retries(retries)
                .backoff(Duration::from_millis(10), Duration::from_millis(20)),
        )
        .build()
        .unwrap()
}

#[tokio::test]
async fn errors_are_visible_and_retry_waits_for_next_poll() {
    for failure in [Mode::Idle, Mode::Disconnect] {
        let server = server(vec![failure, Mode::Good]).await;
        let client = client(&server, 2);
        let mut events = client.subscribe("a".into(), "b".into()).await.unwrap();
        assert!(events.next().await.unwrap().unwrap_err().is_transport());
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
        assert_eq!(events.next().await.unwrap().unwrap(), "recovered");
        assert!(events.next().await.is_none());
        assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn exhausted_budget_is_distinct_and_does_not_reset_on_reconnect() {
    let server = server(vec![Mode::Idle]).await;
    let mut events = client(&server, 2)
        .subscribe("a".into(), "b".into())
        .await
        .unwrap();
    for _ in 0..3 {
        assert!(events.next().await.unwrap().unwrap_err().is_timeout());
    }
    let error = events.next().await.unwrap().unwrap_err();
    assert!(error.is_stream_retry_exhausted());
    assert!(events.next().await.is_none());
    assert!(events.next().await.is_none());
    assert_eq!(server.requests.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn reconnect_setup_failure_is_yielded_before_another_attempt() {
    let server = server(vec![Mode::Idle, Mode::HeadersTimeout, Mode::Good]).await;
    let mut events = client(&server, 2)
        .subscribe("a".into(), "b".into())
        .await
        .unwrap();
    assert!(events.next().await.unwrap().unwrap_err().is_timeout());
    assert!(events.next().await.unwrap().unwrap_err().is_timeout());
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    assert_eq!(events.next().await.unwrap().unwrap(), "recovered");
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn completion_application_errors_and_protocol_errors_do_not_reconnect() {
    for mode in [Mode::End, Mode::Application, Mode::BadJson] {
        let server = server(vec![mode]).await;
        let mut events = client(&server, 3)
            .subscribe("a".into(), "b".into())
            .await
            .unwrap();
        match mode {
            Mode::Application => {
                assert_eq!(
                    events.next().await.unwrap().unwrap_err().code(),
                    Some("busy")
                );
                assert_eq!(events.next().await.unwrap().unwrap(), "recovered");
            }
            Mode::BadJson => assert!(events.next().await.unwrap().unwrap_err().is_protocol()),
            _ => {}
        }
        assert!(events.next().await.is_none());
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn initial_failures_and_reconnect_auth_failures_are_terminal() {
    let server = server(vec![Mode::Denied]).await;
    assert!(
        client(&server, 3)
            .subscribe("a".into(), "b".into())
            .await
            .is_err()
    );
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    drop(server);
    let server = self::server(vec![Mode::Idle, Mode::Denied]).await;
    let mut events = client(&server, 3)
        .subscribe("a".into(), "b".into())
        .await
        .unwrap();
    assert!(events.next().await.unwrap().unwrap_err().is_timeout());
    assert_eq!(
        events.next().await.unwrap().unwrap_err().status(),
        Some(macroni::StatusCode::UNAUTHORIZED)
    );
    assert!(events.next().await.is_none());
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn dropping_during_backoff_cancels_retry() {
    let server = server(vec![Mode::Idle, Mode::Good]).await;
    let client = FeedClient::builder(&server.address)
        .unwrap()
        .bearer_token("test")
        .unwrap()
        .stream_timeout(StreamTimeout::new().idle_timeout(Duration::from_millis(20)))
        .stream_retry(
            StreamRetry::new().backoff(Duration::from_millis(200), Duration::from_millis(200)),
        )
        .build()
        .unwrap();
    let mut events = client.subscribe("a".into(), "b".into()).await.unwrap();
    assert!(events.next().await.unwrap().unwrap_err().is_timeout());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), events.next())
            .await
            .is_err()
    );
    drop(events);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(server.requests.load(Ordering::SeqCst), 1);
}
