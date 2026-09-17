#![cfg(all(feature = "client", feature = "server"))]

mod common;

use axum::{Router, response::IntoResponse, routing::post};
use macroni::{Error, ErrorResponse, Result, StatusCode, api};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    optional: Option<String>,
    count: u32,
}

#[api(client, server)]
pub trait EchoApi {
    #[post("/echo/{id}")]
    #[body(record)]
    async fn echo(&self, id: u32, record: Record) -> Result<Record>;
}

struct EchoService;

#[api]
impl EchoService {
    #[post("/echo/{id}")]
    #[body(record)]
    async fn echo(&self, id: u32, record: Record) -> Result<Record> {
        if id == 0 {
            return Err(Error::bad_request("invalid_id", "ID must be positive")
                .with_details(json!({"id": id})));
        }
        Ok(record)
    }
}

impl EchoApi for EchoService {
    async fn echo(&self, id: u32, record: Record) -> Result<Record> {
        EchoService::echo(self, id, record).await
    }
}

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (address, task)
}

fn record() -> Record {
    Record {
        name: "Bilbo".into(),
        optional: None,
        count: 111,
    }
}

#[tokio::test]
async fn unwrapped_bodies_and_errors_use_the_selected_format() {
    for router in [
        EchoApiServer::router(EchoService),
        EchoServiceServer::router(EchoService),
    ] {
        let (address, server) = serve(router.layer(macroni::server::body_limit(512))).await;
        let client = EchoApiClient::new(&address).unwrap();
        assert_eq!(client.echo(1, record()).await.unwrap(), record());
        let error = client.echo(0, record()).await.unwrap_err();
        assert_eq!(error.status(), Some(StatusCode::BAD_REQUEST));
        let Error::User { response, .. } = error else {
            panic!("expected API error")
        };
        assert_eq!(response.details, Some(json!({"id": 0})));

        let raw = reqwest::Client::new();
        #[cfg(feature = "msgpack")]
        let bytes = rmp_serde::to_vec_named(&record()).unwrap();
        #[cfg(not(feature = "msgpack"))]
        let bytes = serde_json::to_vec(&record()).unwrap();

        let response = raw
            .post(format!("{address}/echo/1"))
            .header("content-type", macroni::__private::CONTENT_TYPE)
            .body(bytes.clone())
            .send()
            .await
            .unwrap();
        // Decode into a map, so a client/server pair using positional structs cannot pass.
        assert_eq!(
            common::decode::<Value>(response).await,
            json!({"name": "Bilbo", "count": 111})
        );

        let response = raw
            .post(format!("{address}/echo/1"))
            .header("content-type", "text/plain")
            .body(bytes.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let error = common::decode::<ErrorResponse>(response).await;
        assert_eq!(
            error.code,
            if cfg!(feature = "msgpack") {
                "invalid_msgpack"
            } else {
                "invalid_json"
            }
        );

        let response = raw
            .post(format!("{address}/echo/not-a-number"))
            .header("content-type", macroni::__private::CONTENT_TYPE)
            .body(bytes)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            common::decode::<ErrorResponse>(response).await.code,
            "invalid_path"
        );

        let oversized = Record {
            name: "x".repeat(1024),
            ..record()
        };
        let error = client.echo(1, oversized).await.unwrap_err();
        assert_eq!(error.status(), Some(StatusCode::PAYLOAD_TOO_LARGE));
        server.abort();
    }
}

#[tokio::test]
async fn client_headers_and_body_match_the_selected_format() {
    let router = Router::new().route(
        "/echo/{id}",
        post(|request: axum::extract::Request| async move {
            let content_type = macroni::__private::CONTENT_TYPE;
            assert_eq!(request.headers()["content-type"], content_type);
            assert_eq!(request.headers()["accept"], content_type);
            let body = axum::body::to_bytes(request.into_body(), 1024)
                .await
                .unwrap();
            #[cfg(feature = "msgpack")]
            let value: Value = rmp_serde::from_slice(&body).unwrap();
            #[cfg(not(feature = "msgpack"))]
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value, json!({"name": "Bilbo", "count": 111}));
            ([("content-type", content_type)], body)
        }),
    );
    let (address, server) = serve(router).await;
    assert_eq!(
        EchoApiClient::new(&address)
            .unwrap()
            .echo(1, record())
            .await
            .unwrap(),
        record()
    );
    server.abort();
}

#[tokio::test]
async fn client_preserves_status_for_wrong_format_and_malformed_responses() {
    for status in [StatusCode::OK, StatusCode::BAD_GATEWAY] {
        for content_type in ["text/plain", macroni::__private::CONTENT_TYPE] {
            let router = Router::new().route(
                "/echo/{id}",
                post(move || async move { (status, [("content-type", content_type)], vec![0xc1]) }),
            );
            let (address, server) = serve(router).await;
            let error = EchoApiClient::new(&address)
                .unwrap()
                .echo(1, record())
                .await
                .unwrap_err();
            assert!(error.is_protocol());
            assert_eq!(error.status(), Some(status));
            server.abort();
        }
    }
}

#[tokio::test]
async fn serialization_failures_are_redacted_in_the_selected_format() {
    struct Fails;
    impl Serialize for Fails {
        fn serialize<S: serde::Serializer>(&self, _: S) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("secret serialization details"))
        }
    }
    let response = macroni::__private::Payload(Fails).into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response.headers()["content-type"],
        macroni::__private::CONTENT_TYPE
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    #[cfg(feature = "msgpack")]
    let error: ErrorResponse = rmp_serde::from_slice(&bytes).unwrap();
    #[cfg(not(feature = "msgpack"))]
    let error: ErrorResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error.code, "internal_server_error");
    assert_eq!(error.message, "An internal server error occurred");

    let error = match macroni::__private::encode_request(
        reqwest::Client::new().post("http://localhost"),
        &Fails,
    ) {
        Ok(_) => panic!("serialization should fail before sending"),
        Err(error) => error,
    };
    assert!(error.is_protocol());
    assert_eq!(error.status(), None);
}
