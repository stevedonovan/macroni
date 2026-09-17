use crate::Error;
use axum::extract::{FromRequest, FromRequestParts};
use axum::http::request::Parts;

pub struct Path<T>(pub T);

impl<S, T> FromRequestParts<S> for Path<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Path::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Path(value)| Self(value))
            .map_err(|error| Error::user(error.status(), "invalid_path", error.body_text()))
    }
}

pub struct Query<T>(pub T);

impl<S, T> FromRequestParts<S> for Query<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(value)| Self(value))
            .map_err(|error| Error::user(error.status(), "invalid_query", error.body_text()))
    }
}

/// Request and response body in the format selected by macroni's features.
pub struct Payload<T>(pub T);

impl<S, T> FromRequest<S> for Payload<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Error;

    async fn from_request(
        request: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        #[cfg(not(feature = "msgpack"))]
        return axum::Json::<T>::from_request(request, state)
            .await
            .map(|axum::Json(value)| Self(value))
            .map_err(|error| Error::user(error.status(), "invalid_json", error.body_text()));

        #[cfg(feature = "msgpack")]
        return axum_serde::MsgPack::<T>::from_request(request, state)
            .await
            .map(|axum_serde::MsgPack(value)| Self(value))
            .map_err(|error| {
                use axum::response::IntoResponse;
                let message = error.to_string();
                Error::user(error.into_response().status(), "invalid_msgpack", message)
            });
    }
}

impl<T: serde::Serialize> axum::response::IntoResponse for Payload<T> {
    fn into_response(self) -> axum::response::Response {
        match crate::codec::encode(&self.0) {
            Ok(bytes) => (
                [(http::header::CONTENT_TYPE, crate::codec::CONTENT_TYPE)],
                bytes,
            )
                .into_response(),
            Err(error) => Error::server("response serialization failed", error).into_response(),
        }
    }
}
