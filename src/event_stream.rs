//! Owned JSON event streams over SSE.
#[cfg(any(feature = "client", feature = "server"))]
use crate::Error;
use crate::Result;
use futures_util::{Stream, StreamExt};
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
#[cfg(feature = "server")]
use tokio::sync::broadcast;
#[cfg(feature = "server")]
use tokio::sync::mpsc;

/// An owned stream of typed events. Dropping it cancels the local subscription.
///
/// Server streams must own their state rather than borrow a handler or lock guard.
/// SSE always uses JSON, even with the `msgpack` feature. Source errors are
/// delivered as items; the source ends the subscription by returning None.
/// Encoding/decoding failures are terminal. Clients may opt into bounded
/// reconnection after connection failures and idle timeouts using StreamRetry.
/// Replay and delivery guarantees remain the application's responsibility.
pub struct EventStream<T> {
    inner: Pin<Box<dyn Stream<Item = Result<T>> + Send + 'static>>,
    heartbeat_interval: Duration,
}

impl<T> EventStream<T> {
    pub fn new(stream: impl Stream<Item = Result<T>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(stream.fuse()),
            heartbeat_interval: Duration::from_secs(20),
        }
    }

    /// Server keep-alive interval. Must be nonzero.
    pub fn heartbeat_interval(mut self, interval: Duration) -> Self {
        assert!(!interval.is_zero(), "heartbeat interval must be nonzero");
        self.heartbeat_interval = interval;
        self
    }
}

#[cfg(feature = "server")]
impl<T> EventStream<T>
where
    T: Clone + Send + 'static,
{
    /// Create an owned subscription without retaining a sender.
    ///
    /// The stream drains and ends only after all strong senders are dropped.
    /// A sender stored in router state therefore keeps subscriptions open;
    /// use a weak sender there when the publishing task should control closure.
    /// Lagging yields a `subscriber_lagged` error; subsequent polls resume at
    /// the oldest retained message.
    pub fn new_from_broadcast(chan: &broadcast::Sender<T>) -> Self {
        let stream = futures_util::stream::unfold(chan.subscribe(), |mut receiver| async move {
            match receiver.recv().await {
                Ok(value) => Some((Ok(value), receiver)),
                Err(broadcast::error::RecvError::Closed) => None,
                Err(broadcast::error::RecvError::Lagged(count)) => Some((
                    Err(crate::Error::user(
                        crate::StatusCode::CONFLICT,
                        "subscriber_lagged",
                        format!("Missed {count} updates"),
                    )),
                    receiver,
                )),
            }
        });
        Self::new(stream)
    }

    pub fn new_from_channel(rx: mpsc::Receiver<Result<T>>) -> Self {
        let stream = futures_util::stream::unfold(rx, |mut rx| async move {
            match rx.recv().await {
                None => None,
                Some(v) => Some((v, rx)),
            }
        });
        Self::new(stream)
    }
}

impl<T> Stream for EventStream<T> {
    type Item = Result<T>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// SSE timeout policy, independent of the overall deadline for ordinary calls.
#[derive(Clone, Copy, Debug)]
pub struct StreamTimeout {
    pub(crate) response_headers: Duration,
    pub(crate) idle: Option<Duration>,
}

impl Default for StreamTimeout {
    fn default() -> Self {
        Self {
            response_headers: Duration::from_secs(10),
            idle: Some(Duration::from_secs(60)),
        }
    }
}

impl StreamTimeout {
    pub fn new() -> Self {
        Self::default()
    }

    /// Deadline from sending the request until response headers arrive.
    pub fn response_headers(mut self, timeout: Duration) -> Self {
        self.response_headers = timeout;
        self
    }

    /// Maximum wait for the next body bytes while consuming the stream.
    /// Heartbeats count as activity. Consumer processing time is not included.
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle = Some(timeout);
        self
    }

    pub fn without_idle_timeout(mut self) -> Self {
        self.idle = None;
        self
    }
}

#[cfg(any(feature = "client", feature = "server"))]
#[derive(serde::Serialize, serde::Deserialize)]
struct StreamError {
    status: u16,
    response: crate::ErrorResponse,
}

#[cfg(feature = "server")]
pub fn sse_response<T: serde::Serialize + 'static>(
    stream: EventStream<T>,
) -> impl axum::response::IntoResponse {
    use axum::response::sse;
    use futures_util::StreamExt;
    let interval = stream.heartbeat_interval;
    let events = futures_util::stream::unfold(Some(stream), |state| async move {
        let mut stream = state?;
        let item = stream.next().await?;
        let source_error = item.is_err();
        let encoded = item.and_then(|item| {
            serde_json::to_string(&item)
                .map_err(|error| Error::server("cannot encode SSE event", error))
        });
        let (event, next) = match encoded {
            Ok(json) => (sse::Event::default().data(json), Some(stream)),
            Err(error) => {
                let error = match error {
                    Error::User { status, response } => StreamError {
                        status: status.as_u16(),
                        response,
                    },
                    error => {
                        tracing::error!(%error, "internal SSE stream error");
                        StreamError {
                            status: 500,
                            response: crate::ErrorResponse {
                                code: "internal_server_error".into(),
                                message: "An internal server error occurred".into(),
                                details: None,
                            },
                        }
                    }
                };
                // ErrorResponse consists entirely of JSON-compatible values.
                let json = serde_json::to_string(&error).expect("serializable stream error");
                (
                    sse::Event::default().event("macroni-error").data(json),
                    source_error.then_some(stream),
                )
            }
        };
        Some((Ok::<_, std::convert::Infallible>(event), next))
    });
    sse::Sse::new(events).keep_alive(sse::KeepAlive::new().interval(interval))
}

#[cfg(feature = "client")]
fn timeout_error(error: tokio::time::error::Elapsed) -> Error {
    Error::Transport {
        source: Box::new(error),
        timeout: true,
    }
}

#[cfg(feature = "client")]
pub async fn open_stream<T: serde::de::DeserializeOwned + 'static>(
    request: reqwest::RequestBuilder,
    timeout: StreamTimeout,
    limit: usize,
) -> Result<EventStream<T>> {
    use http::header::{ACCEPT, CONTENT_TYPE};
    // We time out if the request hasn't arrived by `response_headers` duration
    let response = tokio::time::timeout(
        timeout.response_headers,
        request.header(ACCEPT, "text/event-stream").send(),
    )
    .await
    .map_err(timeout_error)?
    .map_err(Error::from)?;
    if !response.status().is_success() {
        // Error responses before the stream opens use the ordinary wire codec.
        let result = tokio::time::timeout(
            timeout.response_headers,
            crate::client::decode_response::<serde_json::Value>(response, limit),
        )
        .await
        .map_err(timeout_error)?;
        return match result {
            Err(error) => Err(error),
            Ok(_) => Err(Error::protocol(
                None,
                "unexpected successful SSE error response",
                None,
            )),
        };
    }
    let valid = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("text/event-stream")
        });
    if !valid {
        return Err(Error::protocol(
            Some(response.status()),
            "response Content-Type is not text/event-stream",
            None,
        ));
    }
    let state = (
        response,
        BoundedDecoder::new(limit),
        bytes::Bytes::new(),
        0usize,
    );
    let stream = futures_util::stream::unfold(Some(state), move |state| async move {
        let (mut response, mut parser, mut chunk, mut offset) = state?;
        loop {
            while offset < chunk.len() {
                let byte = chunk[offset];
                offset += 1;
                match parser.feed(byte) {
                    Ok(Some((kind, data))) => {
                        let mut application_error = false;
                        let item = match kind.as_str() {
                            "" | "message" => serde_json::from_str::<T>(&data).map_err(|error| {
                                Error::protocol(
                                    None,
                                    format!("invalid SSE JSON event: {error}"),
                                    None,
                                )
                            }),
                            "macroni-error" => {
                                let error = serde_json::from_str::<StreamError>(&data)
                                    .map_err(|error| {
                                        Error::protocol(
                                            None,
                                            format!("invalid SSE error event: {error}"),
                                            None,
                                        )
                                    })
                                    .and_then(|error| {
                                        let status = http::StatusCode::from_u16(error.status)
                                            .map_err(|_| {
                                                Error::protocol(
                                                    None,
                                                    "invalid SSE error status",
                                                    None,
                                                )
                                            })?;
                                        Ok(Error::User {
                                            status,
                                            response: error.response,
                                        })
                                    });
                                application_error = error.is_ok();
                                Err(error.unwrap_or_else(|error| error))
                            }
                            _ => continue,
                        };
                        let next = (item.is_ok() || application_error)
                            .then_some((response, parser, chunk, offset));
                        return Some((item, next));
                    }
                    Ok(None) => {}
                    Err(error) => return Some((Err(error), None)),
                }
            }
            let next = match timeout.idle {
                Some(idle) => match tokio::time::timeout(idle, response.chunk()).await {
                    Ok(result) => result.map_err(Error::from),
                    Err(error) => Err(timeout_error(error)),
                },
                None => response.chunk().await.map_err(Error::from),
            };
            match next {
                Ok(Some(bytes)) => {
                    chunk = bytes;
                    offset = 0;
                }
                Ok(None) => return None,
                Err(error) => return Some((Err(error), None)),
            }
        }
    });
    Ok(EventStream::new(stream))
}

/// Enforce macroni's wire-size and strict UTF-8 policies before handing lines
/// to sse-core. Its own limit covers accumulated field values, not comments or
/// unknown fields, and it deliberately replaces invalid UTF-8 instead of failing.
#[cfg(feature = "client")]
struct BoundedDecoder {
    decoder: sse_core::SseDecoder,
    line: Vec<u8>,
    first_line: bool,
    after_cr: bool,
    size: usize,
    limit: usize,
}

#[cfg(feature = "client")]
impl BoundedDecoder {
    fn new(limit: usize) -> Self {
        Self {
            decoder: sse_core::SseDecoder::with_limit(
                std::num::NonZeroUsize::new(limit.max(1)).unwrap(),
            ),
            line: Vec::new(),
            first_line: true,
            after_cr: false,
            size: 0,
            limit,
        }
    }

    fn feed(&mut self, byte: u8) -> Result<Option<(String, String)>> {
        if self.after_cr {
            self.after_cr = false;
            if byte == b'\n' {
                return Ok(None);
            }
        }
        self.size = self.size.saturating_add(1);
        if self.size > self.limit {
            return Err(Error::protocol(
                None,
                "SSE event exceeds the configured response body limit",
                None,
            ));
        }
        if byte != b'\r' && byte != b'\n' {
            self.line.push(byte);
            return Ok(None);
        }
        self.after_cr = byte == b'\r';
        let line = std::str::from_utf8(&self.line)
            .map_err(|_| Error::protocol(None, "invalid UTF-8 in SSE event", None))?;
        let line = if self.first_line {
            self.first_line = false;
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        if line.is_empty() {
            self.size = 0;
        }
        self.line.push(byte);
        let event = self
            .decoder
            .next(&mut self.line.as_slice())
            .transpose()
            .map_err(|error| Error::protocol(None, format!("invalid SSE event: {error}"), None))?;
        self.line.clear();
        Ok(match event {
            Some(sse_core::SseEvent::Message(event)) => {
                Some((event.event.into_owned(), event.data))
            }
            // Reconnection remains the caller's responsibility.
            Some(sse_core::SseEvent::Retry(_)) | None => None,
        })
    }
}

#[cfg(all(test, feature = "client"))]
mod tests {
    use super::*;

    #[test]
    fn framing_handles_bom_crlf_comments_multiline_and_unknown_fields() {
        let mut parser = BoundedDecoder::new(1024);
        let input = "\u{feff}: heartbeat\r\n\r\ndata: {\rdata: \"value\": \"日本語\"}\rid: 1\rretry: 1000\revent: message\r\r";
        let events: Vec<_> = input
            .bytes()
            .filter_map(|byte| parser.feed(byte).unwrap())
            .collect();
        assert_eq!(
            events,
            vec![("message".into(), "{\n\"value\": \"日本語\"}".into())]
        );
        assert_eq!(parser.size, 0);
    }

    #[test]
    fn bounded_frames_and_utf8_errors() {
        for input in [b"data: 123456".as_slice(), b":12345678901", b"unknown: abc"] {
            let mut parser = BoundedDecoder::new(10);
            assert!(input.iter().any(|byte| parser.feed(*byte).is_err()));
        }
        let mut parser = BoundedDecoder::new(100);
        assert!(parser.feed(0xff).is_ok());
        assert!(parser.feed(b'\n').is_err());
    }

    #[test]
    fn named_events_empty_fields_and_retry_do_not_change_lifecycle() {
        let input = "event: ignored\n\nretry: 20\nid: abc\nevent: macroni-error\ndata: {}\n\n\
                     event: other\nevent\ndata: a\ndata\ndata: b\n\ndata: incomplete";
        let mut decoder = BoundedDecoder::new(256);
        let events: Vec<_> = input
            .bytes()
            .filter_map(|byte| decoder.feed(byte).unwrap())
            .collect();
        assert_eq!(
            events,
            vec![
                ("macroni-error".into(), "{}".into()),
                ("message".into(), "a\n\nb".into()),
            ]
        );
    }

    #[test]
    fn limits_cover_whole_frames_and_reset_between_frames() {
        for input in [
            "data: x\ndata: y\n\n",
            ":123\n:456\n:789",
            "unknown: 123456789",
        ] {
            let mut decoder = BoundedDecoder::new(12);
            assert!(
                input.bytes().any(|byte| decoder.feed(byte).is_err()),
                "{input:?}"
            );
        }
        for ending in ["\n", "\r", "\r\n"] {
            let input = format!("data: x{ending}{ending}").repeat(100);
            let mut decoder = BoundedDecoder::new(9);
            let events: Vec<_> = input
                .bytes()
                .filter_map(|byte| decoder.feed(byte).unwrap())
                .collect();
            assert_eq!(events.len(), 100);
        }
        assert!(BoundedDecoder::new(0).feed(b':').is_err());
    }
}

#[cfg(all(test, feature = "server"))]
mod broadcast_tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_drain_and_end_when_last_strong_sender_drops() {
        let (sender, _) = broadcast::channel(4);
        let weak = sender.downgrade();
        let keep_alive = sender.clone();
        let mut first = EventStream::new_from_broadcast(&sender);
        let mut second = EventStream::new_from_broadcast(&sender);
        sender.send(1).unwrap();
        drop(sender);
        assert_eq!(first.next().await.unwrap().unwrap(), 1);
        assert_eq!(second.next().await.unwrap().unwrap(), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), first.next())
                .await
                .is_err()
        );
        keep_alive.send(2).unwrap();
        drop(keep_alive);
        assert!(weak.upgrade().is_none());
        tokio::time::timeout(Duration::from_secs(1), async {
            for events in [&mut first, &mut second] {
                assert_eq!(events.next().await.unwrap().unwrap(), 2);
                assert!(events.next().await.is_none());
                assert!(events.next().await.is_none());
            }
        })
        .await
        .unwrap();
    }
}
