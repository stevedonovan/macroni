//! Define JSON-over-HTTP APIs with Axum and (optionally) generate a client
//! using Reqwest.
//!
//! [Axum](https://docs.rs/axum]df[) is a powerful and flexible Web framework by the Tokio team built
//! on the lower-level [Hyper](https://docs.rs/hyper) library that can use middleware from
//! the [Tower](https://docs.rs/tower) project.
//!
//! However, a non-trivial `Axum` project requires assembling features from different crates, and
//! doing some customization like making your error type support the `IntoResponse` trait, etc.
//!
//! `macroni` aims to simplify the Axum developer experience for the common case of constructing REST-like JSON APIs.
//! It provides proc macro sugar for generating `Axum` handlers from trait methods or implementation blocks,
//! and sets up `Axum` extractors for you. It provides a suitable error type, and
//! implements custom extractors that use that type, so that errors are formatted as JSON.
//!
//! # Hello World
//! ```rust,no_run
//! use macroni::Result;
//! use macroni::serde_json::{Value, json};
//! use std::sync::Arc;
//!
//! struct Implementation {
//!     id: String,
//! }
//!
//! #[macroni::api]
//! impl Implementation {
//!     #[get("/hello")]
//!     pub async fn hello(&self, name: String, age: u32) -> Result<String> {
//!         Ok(format!(
//!             "Hello {}, {} year old named {}",
//!             self.id, age, name
//!         ))
//!     }
//!
//!     #[post("/hello")]
//!     pub async fn send_hello(&self, name: String, age: u32) -> Result<Value> {
//!         Ok(json!({"user": self.id, "name": name, "age": age}))
//!     }
//!}
//!
//! #[tokio::main]
//! async fn main() {
//!     let server = ImplementationServer::router(Arc::new(Implementation {
//!         id: "Admin".to_owned(),
//!     }));
//!
//!     let address = std::env::var("HELLO_ADDR").unwrap_or_else(|_| "127.0.0.1:3030".into());
//!     let listener = tokio::net::TcpListener::bind(&address)
//!         .await
//!         .expect("bind server listener");
//!
//!     println!("role server listening on http://{address}");
//!     axum::serve(listener, server).await.unwrap();
//! }
//! ```
//! It looks very much like
//! a [minimal Axum example](https://github.com/tokio-rs/axum/blob/main/examples/hello-world/src/main.rs), except
//! instead of handlers as free async functions using extractor patterns, the handlers are generated from ordinary async
//! methods and the
//! arguments of a shared type (like query in case of GET and body in case of POST) are collected into generated structs.
//! The routes themselves
//! are specified as macro attributes, like with the `Rocket` framework.
//!
//! Some conventions are followed when generating the actual `Axum` handlers. For GET handlers, if a
//! parameter is not explicitly from the path, then it comes from the query. Similarly, for a POST handler,
//! if not a path parameter, then it is assumed to be part of the JSON body. So the first method would be evoked as GET
//! `http://localhost:3030/hello?name=Bilbo&age=111` and the POST route will want a body like `{"name":"Bilbo","age":111}`
//!
//! ## Server and Client from a trait
//!
//! If the `macroni::api` macro is applied to an async trait, then a _client implementation_ will be generated
//! for a client program which asks for the "client" feature, and the _server implementation_ must be provided
//! just as before.
//!
//! ```rust,ignore
//! #[macroni::api]
//! pub trait MyApi {
//!     #[get("/role/{name}")]
//!     async fn get_by_name(&self, name: String) -> macroni::Result<Role>;
//!
//!     #[post("/role/{name}")]
//!     async fn post_by_name(&self, name: String, role: Role) -> macroni::Result<()>;
//! }
//! ```
//! (Please see the [examples](https://github.com/stevedonovan/macroni/examples))
//!
#[cfg(feature = "client")]
mod client;
mod error;
#[cfg(feature = "server")]
mod extract;
#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "server")]
pub use axum;
pub use error::{Error, ErrorResponse, Result};
pub use http::StatusCode;
pub use macroni_macros::api;
pub use serde;
pub use serde_json;
#[cfg(feature = "server")]
pub use tokio;

/// A TCP listener
#[cfg(feature = "server")]
pub async fn tcp_listener(address: &str) -> std::io::Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(&address).await
}

/// A Unix Domain Socket (UDS) listener
#[cfg(feature = "server")]
pub async fn unix_listener(socket_path: &str) -> std::io::Result<tokio::net::UnixListener> {
    let socket_path = std::path::PathBuf::from(socket_path);
    if socket_path.exists() {
        let _ = tokio::fs::remove_file(&socket_path).await;
    }

    // Ensure the parent directory exists
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Bind the Unix Domain Socket listener
    tokio::net::UnixListener::bind(&socket_path)
}

/// a convenient macro for starting a default Axum server with either
/// Unix Domain socket or regular TCP/IP address
#[cfg(feature = "server")]
#[macro_export]
macro_rules! serve {
    ($address:expr,$service:expr) => {
        if $address.starts_with("/") {
            let listener = $crate::unix_listener($address)
                .await
                .expect("unable to listen");
            eprintln!("server listening on {}", $address);
            $crate::axum::serve(listener, $service)
                .await
                .expect("unable to serve");
        } else {
            let listener = $crate::tcp_listener($address)
                .await
                .expect("unable to bind");
            eprintln!("server listening on {}", $address);
            $crate::axum::serve(listener, $service)
                .await
                .expect("unable to start server");
        }
    };
}

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

    #[cfg(feature = "client")]
    pub fn parse_url(base_url: &str) -> super::Result<(reqwest::Url, Option<std::path::PathBuf>)> {
        if base_url.starts_with("/") {
            Ok((
                "http://localhost".parse().unwrap(),
                Some(std::path::PathBuf::from(base_url)),
            ))
        } else {
            let base_url = reqwest::Url::parse(base_url).map_err(|error| {
                super::Error::protocol(None, format!("invalid API base URL: {error}"), None)
            })?;
            if !matches!(base_url.scheme(), "http" | "https") || base_url.cannot_be_a_base() {
                return Err(super::Error::protocol(
                    None,
                    "API base URL must be an absolute HTTP or HTTPS URL",
                    None,
                ));
            }
            Ok((base_url, None))
        }
    }
}
