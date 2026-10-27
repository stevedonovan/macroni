//! Enveloped NDJSON uploads. HTTP chunk boundaries have no record semantics.
#[cfg(any(feature = "client", feature = "server"))]
use crate::{Error, ErrorResponse, EventStream, Result, StreamExt};
use std::time::Duration;

/// Independent policies for streaming request bodies.
/// Apply on a server with axum::Extension(config), or on a client builder with
/// .stream_body(config)
#[derive(Clone, Copy, Debug)]
pub struct StreamBodyConfig {
    max_record_bytes: usize,
    heartbeat_interval: Duration,
    idle_timeout: Option<Duration>,
    overall_timeout: Option<Duration>,
}
impl Default for StreamBodyConfig {
    fn default() -> Self {
        Self {
            max_record_bytes: 1024 * 1024,
            heartbeat_interval: Duration::from_secs(15),
            idle_timeout: Some(Duration::from_secs(60)),
            overall_timeout: None,
        }
    }
}
impl StreamBodyConfig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn max_record_bytes(mut self, bytes: usize) -> Self {
        self.max_record_bytes = bytes;
        self
    }
    /// Sender heartbeat interval. Must be nonzero.
    pub fn heartbeat_interval(mut self, interval: Duration) -> Self {
        assert!(!interval.is_zero(), "heartbeat interval must be nonzero");
        self.heartbeat_interval = interval;
        self
    }
    /// Receiver wait for bytes while polling; excludes handler processing time.
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }
    pub fn without_idle_timeout(mut self) -> Self {
        self.idle_timeout = None;
        self
    }
    /// Optional client request deadline and server input-stream deadline.
    pub fn overall_timeout(mut self, timeout: Duration) -> Self {
        self.overall_timeout = Some(timeout);
        self
    }
}

#[cfg(any(feature = "client", feature = "server"))]
const CONTENT_TYPE: &str = "application/x-ndjson";

#[cfg(any(feature = "client", feature = "server"))]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum Record<T> {
    Data { data: T },
    Error { error: ErrorResponse },
    Heartbeat {},
}

#[cfg(feature = "server")]
fn invalid(message: impl Into<String>) -> Error {
    Error::bad_request("invalid_stream_body", message)
}

#[cfg(feature = "client")]
fn timed_out(error: tokio::time::error::Elapsed) -> Error {
    Error::Transport {
        source: Box::new(error),
        timeout: true,
    }
}

#[cfg(feature = "client")]
fn encode<T: serde::Serialize>(record: &Record<T>, limit: usize) -> Result<Vec<u8>> {
    struct Bounded {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other(
                    "stream record exceeds configured limit",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Bounded {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, record).map_err(|error| {
        Error::protocol(None, format!("cannot encode stream record: {error}"), None)
    })?;
    std::io::Write::write_all(&mut output, b"\n")
        .map_err(|error| Error::protocol(None, error.to_string(), None))?;
    Ok(output.bytes)
}

#[cfg(feature = "client")]
pub async fn send_upload<T, R>(
    request: reqwest::RequestBuilder,
    source: EventStream<T>,
    config: &crate::ClientConfig,
) -> Result<R>
where
    T: serde::Serialize + Send + 'static,
    R: serde::de::DeserializeOwned,
{
    let options = config.stream_body();
    let (finished, done) = tokio::sync::oneshot::channel::<()>();
    let body = futures_util::stream::unfold(Some((source, finished)), move |state| async move {
        let (mut source, finished) = state?;
        let record = match tokio::time::timeout(options.heartbeat_interval, source.next()).await {
            Err(_) => Record::Heartbeat {},
            Ok(None) => return None, // Dropping finished starts the response deadline.
            Ok(Some(Ok(data))) => Record::Data { data },
            Ok(Some(Err(error))) => {
                let error = match error {
                    Error::User { response, .. } => response,
                    _ => ErrorResponse {
                        code: "source_error".into(),
                        message: "An internal source error occurred".into(),
                        details: None,
                    },
                };
                Record::Error { error }
            }
        };
        let encoded = encode(&record, options.max_record_bytes);
        let next = encoded.is_ok().then_some((source, finished));
        Some((encoded, next))
    });
    let request = request
        .header(http::header::CONTENT_TYPE, CONTENT_TYPE)
        .header(http::header::ACCEPT, crate::codec::CONTENT_TYPE)
        .body(reqwest::Body::wrap_stream(body));
    let operation = async {
        let send = request.send();
        tokio::pin!(send);
        let response = tokio::select! {
            response = &mut send => response.map_err(Error::from)?,
            _ = done => tokio::time::timeout(config.stream_timeout().response_headers, &mut send)
                .await.map_err(timed_out)?.map_err(Error::from)?,
        };
        tokio::time::timeout(
            config.stream_timeout().response_headers,
            crate::client::decode_response(response, config.response_body_limit()),
        )
        .await
        .map_err(timed_out)?
    };
    match options.overall_timeout {
        Some(duration) => tokio::time::timeout(duration, operation)
            .await
            .map_err(timed_out)?,
        None => operation.await,
    }
}

#[cfg(feature = "server")]
pub struct StreamingBody<T>(pub T);

#[cfg(feature = "server")]
impl<S, T> axum::extract::FromRequest<S> for StreamingBody<EventStream<T>>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned + Send + 'static,
{
    type Rejection = Error;
    async fn from_request(request: axum::extract::Request, _: &S) -> Result<Self> {
        if !crate::headers::has_content_type(request.headers(), CONTENT_TYPE) {
            return Err(Error::user(
                http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "invalid_stream_content_type",
                "expected application/x-ndjson",
            ));
        }
        let config = request
            .extensions()
            .get::<StreamBodyConfig>()
            .copied()
            .unwrap_or_default();
        Ok(Self(decode_body(request.into_body(), config)))
    }
}

#[cfg(feature = "server")]
fn decode_body<T: serde::de::DeserializeOwned + Send + 'static>(
    body: axum::body::Body,
    config: StreamBodyConfig,
) -> EventStream<T> {
    let deadline = config
        .overall_timeout
        .map(|timeout| tokio::time::Instant::now() + timeout);
    let state = (
        body.into_data_stream(),
        bytes::Bytes::new(),
        0usize,
        Vec::new(),
    );
    EventStream::new(futures_util::stream::unfold(
        Some(state),
        move |state| async move {
            let (mut body, mut chunk, mut offset, mut line) = state?;
            loop {
                if let Some(deadline) = deadline {
                    if tokio::time::Instant::now() >= deadline {
                        return Some((
                            Err(Error::user(
                                http::StatusCode::REQUEST_TIMEOUT,
                                "stream_body_timeout",
                                "stream body deadline exceeded",
                            )),
                            None,
                        ));
                    }
                }
                while offset < chunk.len() {
                    let byte = chunk[offset];
                    offset += 1;
                    if line.len() >= config.max_record_bytes {
                        return Some((
                            Err(Error::user(
                                http::StatusCode::PAYLOAD_TOO_LARGE,
                                "stream_record_too_large",
                                "stream record exceeds configured limit",
                            )),
                            None,
                        ));
                    }
                    line.push(byte);
                    if byte == b'\n' {
                        let record = serde_json::from_slice::<Record<T>>(&line);
                        line.clear();
                        let item = match record {
                            Ok(Record::Data { data }) => Ok(data),
                            Ok(Record::Error { error }) => Err(Error::User {
                                status: http::StatusCode::BAD_REQUEST,
                                response: error,
                            }),
                            Ok(Record::Heartbeat {}) => continue,
                            Err(error) => {
                                return Some((
                                    Err(invalid(format!("invalid NDJSON envelope: {error}"))),
                                    None,
                                ));
                            }
                        };
                        return Some((item, Some((body, chunk, offset, line))));
                    }
                }
                let wait =
                    async {
                        match config.idle_timeout {
                            Some(timeout) => tokio::time::timeout(timeout, body.next())
                                .await
                                .map_err(|_| {
                                    Error::user(
                                        http::StatusCode::REQUEST_TIMEOUT,
                                        "stream_body_timeout",
                                        "stream body idle timeout",
                                    )
                                }),
                            None => Ok(body.next().await),
                        }
                    };
                let next = match deadline {
                    Some(deadline) => tokio::time::timeout_at(deadline, wait)
                        .await
                        .unwrap_or_else(|_| {
                            Err(Error::user(
                                http::StatusCode::REQUEST_TIMEOUT,
                                "stream_body_timeout",
                                "stream body deadline exceeded",
                            ))
                        }),
                    None => wait.await,
                };
                match next {
                    Ok(Some(Ok(bytes))) => {
                        chunk = bytes;
                        offset = 0;
                    }
                    Ok(None) if line.is_empty() => return None,
                    Ok(None) => {
                        return Some((
                            Err(invalid("incomplete NDJSON record at end of body")),
                            None,
                        ));
                    }
                    Ok(Some(Err(error))) => {
                        return Some((
                            Err(invalid(format!("interrupted stream body: {error}"))),
                            None,
                        ));
                    }
                    Err(error) => return Some((Err(error), None)),
                }
            }
        },
    ))
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use futures_util::stream;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[tokio::test]
    async fn body_is_only_polled_on_demand_and_drop_cancels() {
        let polls = Arc::new(AtomicUsize::new(0));
        let count = polls.clone();
        let source = stream::poll_fn(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(bytes::Bytes::from_static(
                b"{\"type\":\"data\",\"data\":1}\n",
            ))))
        });
        let mut decoded = decode_body::<u32>(
            axum::body::Body::from_stream(source),
            StreamBodyConfig::new(),
        );
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert_eq!(decoded.next().await.unwrap().unwrap(), 1);
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        drop(decoded);
        tokio::task::yield_now().await;
        assert_eq!(polls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn interrupted_body_is_terminal_and_processing_time_is_not_idle_time() {
        let body = axum::body::Body::from_stream(stream::iter([
            Ok(bytes::Bytes::from_static(
                b"{\"type\":\"data\",\"data\":1}\n",
            )),
            Err(std::io::Error::other("interrupted")),
        ]));
        let mut decoded = decode_body::<u32>(
            body,
            StreamBodyConfig::new().idle_timeout(Duration::from_millis(5)),
        );
        assert_eq!(decoded.next().await.unwrap().unwrap(), 1);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            decoded
                .next()
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("interrupted")
        );
        assert!(decoded.next().await.is_none());
    }
}
