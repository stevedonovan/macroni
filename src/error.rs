#[cfg(feature = "server")]
use axum::Json;
#[cfg(feature = "server")]
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::{self, Display};

pub type Result<T> = std::result::Result<T, Error>;

/// `ErrorResponse` is the form of the error that is passed as a response, together with
/// the status code
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// `Error` distinguishes between user errors, server errors, transport errors
/// and protocol errors.
/// On the client side, Reqwest errors become transport errors,
/// and on the server side, errors can be converted into responses
#[derive(Debug)]
pub enum Error {
    User {
        status: StatusCode,
        response: ErrorResponse,
    },
    Server {
        message: String,
        body: Option<String>,
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
    /// this is the most customizable error available.
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

    /// add more structured details to a user error
    /// e.g. `Error::user(StatusCode::BadRequest,"bad","wrong parm").with_details(json!({"parm":"health}))`
    pub fn with_details(mut self, details: Value) -> Self {
        if let Self::User { response, .. } = &mut self {
            response.details = Some(details);
        }
        self
    }

    /// All server errors render as status 500. For other codes, use `user`
    pub fn server(msg: impl ToString, description: impl ToString) -> Self {
        Self::Server {
            message: msg.to_string(),
            body: Some(description.to_string()),
        }
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

    /// HTTP status of this error (note protocol errors do not have status)
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Self::User { status, .. } => Some(*status),
            Self::Protocol { status, .. } => *status,
            Self::Server { .. } => Some(StatusCode::INTERNAL_SERVER_ERROR),
            Self::Transport { .. } => None,
        }
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            Self::User { response, .. } => Some(&response.code),
            _ => None,
        }
    }

    /// the request has timed out
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Transport { timeout: true, .. })
    }

    /// the endpoint has decided to time out
    pub fn is_request_timeout(&self) -> bool {
        matches!(self, Self::User {status,..} if *status == StatusCode::REQUEST_TIMEOUT)
    }

    pub fn is_transport(&self) -> bool {
        matches!(self, Self::Transport { .. })
    }

    pub fn is_server(&self) -> bool {
        matches!(self, Self::Server { .. } | Self::User { .. })
    }

    pub fn is_protocol(&self) -> bool {
        matches!(self, Self::Protocol { .. })
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User { response, .. } => write!(f, "{}: {}", response.code, response.message),
            Self::Server { message, body } => {
                write!(f, "{}: {}", message, body.clone().unwrap_or_default())
            }
            Self::Transport { source, .. } => Display::fmt(source, f),
            Self::Protocol { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
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
            Self::Server { message, body } => {
                let body = body.unwrap_or_default();
                tracing::error!(error = %message, body = %body, "internal API error");
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
