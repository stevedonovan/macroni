//! Consumer-driven reconnection for established subscriptions.
use crate::{Error, EventStream, Result, StreamExt, StreamTimeout};
use std::time::Duration;

/// Opt-in reconnection policy. Defaults to three retries over the entire
/// subscription, with exponential backoff from 200ms to 30s and jitter.
/// Successful connections/events do not reset the budget.
#[derive(Clone, Copy, Debug)]
pub struct StreamRetry(sse_core::SseRetryConfig);

impl Default for StreamRetry {
    fn default() -> Self {
        Self(sse_core::SseRetryConfig {
            max_retries: 3,
            max_backoff_ms: 30_000,
            ..sse_core::SseRetryConfig::new()
        })
    }
}

impl StreamRetry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Maximum reconnection attempts, excluding the initial subscription.
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.0.max_retries = retries;
        self
    }

    /// Configure the base and maximum delay. Jitter chooses between the base
    /// and capped exponential delay. Server retry directives are ignored.
    ///
    /// Panics unless both durations are whole milliseconds in 1..=u32::MAX
    /// and the maximum is at least the initial delay.
    pub fn backoff(mut self, initial: Duration, maximum: Duration) -> Self {
        fn millis(value: Duration) -> u32 {
            let ms = u32::try_from(value.as_millis()).expect("backoff exceeds u32 milliseconds");
            assert!(
                ms > 0 && value == Duration::from_millis(ms.into()),
                "backoff must be a positive whole number of milliseconds"
            );
            ms
        }
        self.0.min_sleep_ms = millis(initial);
        self.0.max_backoff_ms = millis(maximum);
        assert!(
            self.0.min_sleep_ms <= self.0.max_backoff_ms,
            "maximum backoff is less than initial"
        );
        self
    }
}

/// Final error after an established subscription consumes its retry budget.
/// Wrapped in Error::Transport; use Error::is_stream_retry_exhausted().
#[derive(Debug)]
pub struct StreamRetryExhausted {
    pub attempts: u32,
}
impl std::fmt::Display for StreamRetryExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "stream retry budget exhausted after {} reconnection attempts",
            self.attempts
        )
    }
}
impl std::error::Error for StreamRetryExhausted {}

fn retryable(error: &Error) -> bool {
    let Error::Transport { source, timeout } = error else {
        return false;
    };
    if *timeout {
        return true;
    }
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(source.as_ref());
    while let Some(error) = cause {
        // Reqwest can classify a truncated HTTP body as a decode error. Inspect
        // its causes rather than retrying every decode/protocol failure.
        if let Some(error) = error.downcast_ref::<hyper::Error>() {
            if error.is_incomplete_message() || error.is_closed() || error.is_canceled() {
                return true;
            }
        }
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind::*;
            if matches!(
                error.kind(),
                ConnectionRefused
                    | ConnectionReset
                    | ConnectionAborted
                    | BrokenPipe
                    | UnexpectedEof
                    | TimedOut
                    | NotConnected
            ) {
                return true;
            }
        }
        cause = error.source();
    }
    false
}

pub async fn open_stream<T: serde::de::DeserializeOwned + 'static>(
    request: reqwest::RequestBuilder,
    timeout: StreamTimeout,
    limit: usize,
    retry: Option<StreamRetry>,
) -> Result<EventStream<T>> {
    let Some(policy) = retry else {
        return crate::event_stream::open_stream(request, timeout, limit).await;
    };
    // Generated SSE endpoints are GET requests. Capture the complete request,
    // preserving URL, query parameters, headers, and the shared HTTP client.
    let template = request.try_clone().ok_or_else(|| {
        Error::protocol(None, "stream retries require a replayable request", None)
    })?;
    // Initial setup failures remain errors from subscribe().await.
    let active = crate::event_stream::open_stream(request, timeout, limit).await?;
    let stream = futures_util::stream::unfold(
        Some((Some(active), template, 0u32)),
        move |state| async move {
            let (mut active, template, mut attempts) = state?;
            if active.is_none() {
                let Some(delay) = policy.0.calculate_backoff(policy.0.min_sleep_ms, attempts)
                else {
                    return Some((
                        Err(Error::transport(StreamRetryExhausted { attempts })),
                        None,
                    ));
                };
                tokio::time::sleep(delay).await;
                attempts += 1;
                let request = template.try_clone().expect("validated replayable request");
                match crate::event_stream::open_stream(request, timeout, limit).await {
                    Ok(stream) => active = Some(stream),
                    Err(error) => {
                        let next = retryable(&error).then_some((None, template, attempts));
                        return Some((Err(error), next));
                    }
                }
            }
            let item = active.as_mut().unwrap().next().await?;
            let next = match &item {
                Err(error) if retryable(error) => Some((None, template, attempts)),
                Err(Error::User { .. }) | Ok(_) => Some((active, template, attempts)),
                Err(_) => None,
            };
            // No sleep or connection attempt occurs until the next poll.
            Some((item, next))
        },
    );
    Ok(EventStream::new(stream))
}
