//! Define JSON-over-HTTP APIs once and use them from both Axum and Reqwest.

#[cfg(feature = "client")]
mod client;
mod error;
#[cfg(feature = "server")]
mod extract;
#[cfg(feature = "server")]
pub mod server;

pub use error::{Error, ErrorResponse, Result};
pub use http::StatusCode;
pub use macroni_macros::api;

#[doc(hidden)]
pub mod __private {
    #[cfg(feature = "server")]
    pub use axum;
    #[cfg(feature = "client")]
    pub use reqwest;
    pub use serde;

    #[cfg(feature = "client")]
    pub use crate::client::decode_response;
    #[cfg(feature = "server")]
    pub use crate::extract::{Json, Path, Query};

    #[cfg(feature = "client")]
    pub fn encode_path_segment(value: &str) -> String {
        const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
            .add(b' ')
            .add(b'"')
            .add(b'#')
            .add(b'%')
            .add(b'/')
            .add(b'<')
            .add(b'>')
            .add(b'?')
            .add(b'`')
            .add(b'{')
            .add(b'}');

        percent_encoding::utf8_percent_encode(value, PATH_SEGMENT).to_string()
    }
}
