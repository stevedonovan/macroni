use crate::{Error, ErrorResponse, Result};
use http::header::CONTENT_TYPE;
use reqwest::Response;
use serde::de::DeserializeOwned;

const MAX_ERROR_EXCERPT_BYTES: usize = 4 * 1024;

pub async fn decode_response<T: DeserializeOwned>(
    mut response: Response,
    max_response_bytes: usize,
) -> Result<T> {
    let status = response.status();
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let media_type = value.split(';').next().unwrap_or_default().trim();
            media_type == "application/json" || media_type.ends_with("+json")
        });

    if !is_json {
        return Err(Error::protocol(
            Some(status),
            "response Content-Type is not JSON",
            None,
        ));
    }

    if response
        .content_length()
        .is_some_and(|length| length > max_response_bytes as u64)
    {
        return Err(Error::protocol(
            Some(status),
            "response body exceeds the configured limit",
            None,
        ));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Error::from)? {
        if body.len().saturating_add(chunk.len()) > max_response_bytes {
            return Err(Error::protocol(
                Some(status),
                "response body exceeds the configured limit",
                None,
            ));
        }
        body.extend_from_slice(&chunk);
    }

    if status.is_success() {
        serde_json::from_slice(&body).map_err(|error| {
            Error::protocol(
                Some(status),
                format!("invalid JSON success response: {error}"),
                body_excerpt(&body),
            )
        })
    } else {
        match serde_json::from_slice::<ErrorResponse>(&body) {
            Ok(response) => Err(Error::Remote { status, response }),
            Err(error) => Err(Error::protocol(
                Some(status),
                format!("invalid JSON error response: {error}"),
                body_excerpt(&body),
            )),
        }
    }
}

fn body_excerpt(body: &[u8]) -> Option<String> {
    let end = body.len().min(MAX_ERROR_EXCERPT_BYTES);
    (!body.is_empty()).then(|| String::from_utf8_lossy(&body[..end]).into_owned())
}
