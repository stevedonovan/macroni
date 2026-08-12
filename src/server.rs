//! Small middleware helpers that preserve Macaroni's JSON error protocol.

use crate::Error;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::time::Duration;

/// Apply with `axum::middleware::from_fn_with_state(duration, timeout)`.
pub async fn timeout(State(duration): State<Duration>, request: Request, next: Next) -> Response {
    match tokio::time::timeout(duration, next.run(request)).await {
        Ok(response) => response,
        Err(_) => Error::user(
            http::StatusCode::GATEWAY_TIMEOUT,
            "request_timeout",
            "The request exceeded its deadline",
        )
        .into_response(),
    }
}

/// Configure Axum's request body limit. Extractor failures remain JSON responses.
pub fn body_limit(max_bytes: usize) -> axum::extract::DefaultBodyLimit {
    axum::extract::DefaultBodyLimit::max(max_bytes)
}
