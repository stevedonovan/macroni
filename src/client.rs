use crate::{Error, ErrorResponse, Result, codec};
use http::header::CONTENT_TYPE;
use reqwest::Response;
use serde::de::DeserializeOwned;

const MAX_ERROR_EXCERPT_BYTES: usize = 4 * 1024;

pub fn encode_request<T: serde::Serialize + ?Sized>(
    request: reqwest::RequestBuilder,
    payload: &T,
) -> Result<reqwest::RequestBuilder> {
    let body = codec::encode(payload).map_err(|error| {
        Error::protocol(
            None,
            format!("cannot encode {} request: {error}", codec::FORMAT),
            None,
        )
    })?;
    Ok(request.header(CONTENT_TYPE, codec::CONTENT_TYPE).body(body))
}

pub async fn decode_response<T: DeserializeOwned>(
    mut response: Response,
    max_response_bytes: usize,
) -> Result<T> {
    let status = response.status();
    let supported_content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(codec::matches_content_type);

    if !supported_content_type {
        return Err(Error::protocol(
            Some(status),
            format!("response Content-Type is not {}", codec::FORMAT),
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
        codec::decode(&body).map_err(|error| {
            Error::protocol(
                Some(status),
                format!("invalid {} success response: {error}", codec::FORMAT),
                body_excerpt(&body),
            )
        })
    } else {
        match codec::decode::<ErrorResponse>(&body) {
            Ok(response) => Err(Error::User { status, response }),
            Err(error) => Err(Error::protocol(
                Some(status),
                format!("invalid {} error response: {error}", codec::FORMAT),
                body_excerpt(&body),
            )),
        }
    }
}

fn body_excerpt(body: &[u8]) -> Option<String> {
    let end = body.len().min(MAX_ERROR_EXCERPT_BYTES);
    (!body.is_empty()).then(|| String::from_utf8_lossy(&body[..end]).into_owned())
}
