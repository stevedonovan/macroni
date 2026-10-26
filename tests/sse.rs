#![cfg(all(feature = "client", feature = "server"))]

use futures_util::{StreamExt, stream};
use macroni::{EventStream, Result, StreamTimeout};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[macroni::api]
pub trait Events {
    #[get("/events/{topic}")]
    #[sse]
    async fn subscribe(&mut self, topic: String) -> Result<EventStream<String>>;

    #[get("/slow")]
    async fn slow(&self) -> Result<String>;
}

struct DropFlag(Arc<AtomicBool>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Service(Arc<AtomicBool>);
impl Events for Service {
    async fn subscribe(&mut self, topic: String) -> Result<EventStream<String>> {
        match topic.as_str() {
            "denied" => Err(macroni::Error::bad_request("denied", "not allowed")),
            "setup" => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(EventStream::new(stream::empty()))
            }
            "heartbeat" | "idle" => {
                let heartbeat = if topic == "heartbeat" {
                    Duration::from_millis(15)
                } else {
                    Duration::from_secs(15)
                };
                let flag = DropFlag(self.0.clone());
                let stream = stream::unfold(Some(flag), |state| async move {
                    let flag = state?;
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    drop(flag);
                    Some((Ok("alive".into()), None))
                });
                Ok(EventStream::new(stream).heartbeat_interval(heartbeat))
            }
            "broadcast" => {
                let (sender, _) = tokio::sync::broadcast::channel(2);
                let events = EventStream::new_from_broadcast(&sender);
                for value in ["lost", "retained", "latest"] {
                    sender.send(value.to_owned()).unwrap();
                }
                Ok(events)
            }
            "fatal" => Ok(EventStream::new(stream::iter([Err(
                macroni::Error::bad_request("finished", "source ended"),
            )]))),
            "internal" => Ok(EventStream::new(stream::iter([
                Err(macroni::Error::server("SECRET", "private detail")),
                Ok("recovered".into()),
            ]))),
            _ => Ok(EventStream::new(stream::iter([
                Ok("hello 日本語\nworld".into()),
                Err(macroni::Error::bad_request(
                    "lagged",
                    "subscriber fell behind",
                )),
                Ok("recovered".into()),
            ]))),
        }
    }

    async fn slow(&self) -> Result<String> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok("done".into())
    }
}

async fn server() -> (String, tokio::task::JoinHandle<()>, Arc<AtomicBool>) {
    let listener = macroni::tcp_listener("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let dropped = Arc::new(AtomicBool::new(false));
    let router = EventsServer::router(Service(dropped.clone()));
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (address, task, dropped)
}

#[tokio::test]
async fn json_events_continue_after_source_errors() {
    let (address, task, _) = server().await;
    let mut client = EventsClient::new(&address).unwrap();
    let mut events = client.subscribe("normal".into()).await.unwrap();
    assert_eq!(events.next().await.unwrap().unwrap(), "hello 日本語\nworld");
    let error = events.next().await.unwrap().unwrap_err();
    assert_eq!(error.code(), Some("lagged"));
    assert_eq!(events.next().await.unwrap().unwrap(), "recovered");
    assert!(events.next().await.is_none());
    assert!(events.next().await.is_none());
    let error = client.subscribe("denied".into()).await.err().unwrap();
    assert_eq!(error.code(), Some("denied"));
    let mut events = client.subscribe("internal".into()).await.unwrap();
    let error = events.next().await.unwrap().unwrap_err();
    assert_eq!(error.code(), Some("internal_server_error"));
    assert!(!error.to_string().contains("SECRET"));
    assert_eq!(events.next().await.unwrap().unwrap(), "recovered");
    assert!(events.next().await.is_none());
    let mut events = client.subscribe("fatal".into()).await.unwrap();
    assert_eq!(
        events.next().await.unwrap().unwrap_err().code(),
        Some("finished")
    );
    assert!(events.next().await.is_none());

    let mut events = client.subscribe("broadcast".into()).await.unwrap();
    assert_eq!(
        events.next().await.unwrap().unwrap_err().code(),
        Some("subscriber_lagged")
    );
    assert_eq!(events.next().await.unwrap().unwrap(), "retained");
    assert_eq!(events.next().await.unwrap().unwrap(), "latest");
    assert!(events.next().await.is_none());
    task.abort();
}

#[tokio::test]
async fn heartbeat_keeps_stream_alive_without_holding_state_lock() {
    let (address, task, _) = server().await;
    let mut client = EventsClient::builder(&address)
        .unwrap()
        .timeout(Duration::from_millis(30))
        .stream_timeout(StreamTimeout::new().idle_timeout(Duration::from_millis(100)))
        .build()
        .unwrap();
    let mut events = client.subscribe("heartbeat".into()).await.unwrap();
    // A second mutable subscription must acquire the state lock immediately.
    assert!(
        client
            .subscribe("denied".into())
            .await
            .err()
            .unwrap()
            .code()
            .is_some()
    );
    // Finite requests still observe their overall deadline.
    assert!(client.slow().await.unwrap_err().is_timeout());
    assert_eq!(events.next().await.unwrap().unwrap(), "alive");
    assert!(events.next().await.is_none());
    task.abort();
}

#[tokio::test]
async fn idle_and_header_deadlines_are_terminal() {
    let (address, task, _) = server().await;
    let mut client = EventsClient::builder(&address)
        .unwrap()
        .stream_timeout(
            StreamTimeout::new()
                .response_headers(Duration::from_millis(50))
                .idle_timeout(Duration::from_millis(50)),
        )
        .build()
        .unwrap();
    let mut events = client.subscribe("idle".into()).await.unwrap();
    assert!(events.next().await.unwrap().unwrap_err().is_timeout());
    assert!(events.next().await.is_none());
    assert!(
        client
            .subscribe("setup".into())
            .await
            .err()
            .unwrap()
            .is_timeout()
    );
    task.abort();
}

#[tokio::test]
async fn dropping_client_stream_releases_server_subscription() {
    let (address, task, dropped) = server().await;
    let mut client = EventsClient::new(&address).unwrap();
    let events = client.subscribe("heartbeat".into()).await.unwrap();
    drop(events);
    tokio::time::timeout(Duration::from_millis(200), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
}

#[macroni::api]
impl Direct {
    #[get("/direct")]
    #[sse]
    pub async fn events(&self) -> Result<EventStream<u32>> {
        Ok(EventStream::new(stream::iter([Ok(42)])))
    }
}
struct Direct;

#[tokio::test]
async fn direct_impl_produces_sse() {
    let listener = macroni::tcp_listener("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/direct", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, DirectServer::router(Direct))
            .await
            .unwrap()
    });
    let response = reqwest::get(address).await.unwrap();
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.text().await.unwrap(), "data: 42\n\n");
    task.abort();
}

#[tokio::test]
async fn client_validates_framing_content_type_and_per_event_size() {
    use axum::{body::Body, response::Response};
    let router = axum::Router::new().route(
        "/events/{topic}",
        axum::routing::get(
            |axum::extract::Path(topic): axum::extract::Path<String>| async move {
                let (content_type, bytes): (&str, Vec<u8>) = match topic.as_str() {
                    "wrong-type" => ("application/json", b"42".to_vec()),
                    "bad-error" => ("text/event-stream", b"event: macroni-error\ndata: {}\n\ndata: \"ignored\"\n\n".to_vec()),
                    "bad-status" => ("text/event-stream", b"event: macroni-error\ndata: {\"status\":0,\"response\":{\"code\":\"x\",\"message\":\"x\"}}\n\n".to_vec()),
                    "bad-json" => ("text/event-stream", b"data: nope\n\n".to_vec()),
                    "bad-utf8" => ("text/event-stream", vec![0xff, b'\n']),
                    "large" => (
                        "text/event-stream",
                        format!("data: \"{}\"\n\n", "x".repeat(200)).into_bytes(),
                    ),
                    "many" => (
                        "text/event-stream",
                        "data: \"ok\"\n\n".repeat(100).into_bytes(),
                    ),
                    _ => (
                        "text/event-stream",
                        "\u{feff}: ping\r\n\r\nevent: message\r\ndata: \"日本語\"\r\n\r\n"
                            .as_bytes()
                            .to_vec(),
                    ),
                };
                // Split UTF-8 and frame delimiters across body chunks.
                let chunks = stream::iter(
                    bytes
                        .into_iter()
                        .map(|byte| Ok::<_, std::convert::Infallible>(vec![byte])),
                );
                Response::builder()
                    .header("content-type", content_type)
                    .body(Body::from_stream(chunks))
                    .unwrap()
            },
        ),
    );
    let listener = macroni::tcp_listener("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut client = EventsClient::builder(&address)
        .unwrap()
        .response_body_limit(128)
        .build()
        .unwrap();
    assert!(
        client
            .subscribe("wrong-type".into())
            .await
            .err()
            .unwrap()
            .is_protocol()
    );
    for topic in ["bad-json", "bad-error", "bad-status", "bad-utf8", "large"] {
        let mut events = client.subscribe(topic.into()).await.unwrap();
        assert!(events.next().await.unwrap().unwrap_err().is_protocol());
        assert!(events.next().await.is_none());
    }
    let mut events = client.subscribe("many".into()).await.unwrap();
    for _ in 0..100 {
        assert_eq!(events.next().await.unwrap().unwrap(), "ok");
    }
    assert!(events.next().await.is_none());
    let mut events = client.subscribe("fragmented".into()).await.unwrap();
    assert_eq!(events.next().await.unwrap().unwrap(), "日本語");
    assert!(events.next().await.is_none());
    task.abort();
}
