#[cfg(feature = "server")]
use axum::Json;
#[cfg(feature = "server")]
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::{self, Display};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug)]
pub enum Error {
    User {
        status: StatusCode,
        response: ErrorResponse,
    },
    Internal(anyhow::Error),
    Remote {
        status: StatusCode,
        response: ErrorResponse,
    },
    Transport {
        source: Box<dyn std::error::Error + Send + Sync>,
        timeout: bool,
    },
    Protocol {
        status: Option<StatusCode>,
        message: String,
        body: Option<String>,
    },
}

impl Error {
    pub fn user(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::User {
            status,
            response: ErrorResponse {
                code: code.into(),
                message: message.into(),
                details: None,
            },
        }
    }

    pub fn bad_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::user(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn not_found(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::user(StatusCode::NOT_FOUND, code, message)
    }

    pub fn with_details(mut self, details: Value) -> Self {
        if let Self::User { response, .. } = &mut self {
            response.details = Some(details);
        }
        self
    }

    pub fn internal(error: impl Into<anyhow::Error>) -> Self {
        Self::Internal(error.into())
    }

    pub fn protocol(
        status: Option<StatusCode>,
        message: impl Into<String>,
        body: Option<String>,
    ) -> Self {
        Self::Protocol {
            status,
            message: message.into(),
            body,
        }
    }

    pub fn transport(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Transport {
            source: Box::new(error),
            timeout: false,
        }
    }

    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Self::User { status, .. } | Self::Remote { status, .. } => Some(*status),
            Self::Protocol { status, .. } => *status,
            Self::Internal(_) | Self::Transport { .. } => None,
        }
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            Self::User { response, .. } | Self::Remote { response, .. } => Some(&response.code),
            _ => None,
        }
    }

    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Transport { timeout: true, .. })
    }

    pub fn is_transport(&self) -> bool {
        matches!(self, Self::Transport { .. })
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    pub fn is_protocol(&self) -> bool {
        matches!(self, Self::Protocol { .. })
    }

    pub fn body_excerpt(&self) -> Option<&str> {
        match self {
            Self::Protocol { body, .. } => body.as_deref(),
            _ => None,
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User { response, .. } => write!(f, "{}: {}", response.code, response.message),
            Self::Remote { response, .. } => {
                write!(f, "remote {}: {}", response.code, response.message)
            }
            Self::Internal(error) => Display::fmt(error, f),
            Self::Transport { source, .. } => Display::fmt(source, f),
            Self::Protocol { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Internal(error) => Some(error.as_ref()),
            Self::Transport { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

#[cfg(feature = "client")]
impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        let timeout = error.is_timeout();
        Self::Transport {
            source: Box::new(error),
            timeout,
        }
    }
}

#[cfg(feature = "server")]
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Self::User { status, response } => (status, Json(response)).into_response(),
            Self::Internal(error) => {
                tracing::error!(error = %error, "internal API error");
                internal_response()
            }
            Self::Remote {
                status: _,
                response,
            } => {
                tracing::error!(code = %response.code, "unexpected remote error in API server");
                internal_response()
            }
            Self::Transport { source, .. } => {
                tracing::error!(error = %source, "unexpected transport error in API server");
                internal_response()
            }
            Self::Protocol { message, .. } => {
                tracing::error!(%message, "API protocol error");
                internal_response()
            }
        }
    }
}

#[cfg(feature = "server")]
fn internal_response() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            code: "internal_server_error".into(),
            message: "An internal server error occurred".into(),
            details: None,
        }),
    )
        .into_response()
}
