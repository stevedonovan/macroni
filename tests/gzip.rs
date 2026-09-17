#![cfg(all(feature = "client", feature = "server"))]

mod common;

use macroni::{Result, api};

fn payload() -> String {
    "compressible response ".repeat(1024)
}

#[api(client, server)]
pub trait PayloadApi {
    #[get("/payload")]
    async fn payload(&self) -> Result<String>;
}

struct PayloadService;

impl PayloadApi for PayloadService {
    async fn payload(&self) -> Result<String> {
        Ok(payload())
    }
}

struct DirectService;

#[api]
impl DirectService {
    #[get("/payload")]
    async fn payload(&self) -> Result<String> {
        Ok(payload())
    }
}

#[tokio::test]
async fn generated_routers_negotiate_compression() {
    // Exercise both expansion paths without any gzip feature on the API declaration.
    for router in [
        PayloadApiServer::router(PayloadService),
        DirectServiceServer::router(DirectService),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let raw = reqwest::Client::builder().no_gzip().build().unwrap();

        for encoding in [None, Some("identity"), Some("gzip;q=0")] {
            let mut request = raw.get(format!("{address}/payload"));
            if let Some(encoding) = encoding {
                request = request.header("accept-encoding", encoding);
            }
            let response = request.send().await.unwrap();
            assert!(response.headers().get("content-encoding").is_none());
            assert_eq!(common::decode::<String>(response).await, payload());
        }

        let response = raw
            .get(format!("{address}/payload"))
            .header("accept-encoding", "gzip")
            .send()
            .await
            .unwrap();
        if cfg!(feature = "gzip") {
            assert_eq!(response.headers()["content-encoding"], "gzip");
            let compressed = response.bytes().await.unwrap();
            assert!(compressed.starts_with(&[0x1f, 0x8b]));
            assert!(compressed.len() < 1024);
        } else {
            assert!(response.headers().get("content-encoding").is_none());
            assert_eq!(common::decode::<String>(response).await, payload());
        }

        let client = PayloadApiClient::new(&address).unwrap();
        assert_eq!(client.payload().await.unwrap(), payload());

        // The compressed response is below this limit, but the decoded body is above it.
        let limited = PayloadApiClient::builder(&address)
            .unwrap()
            .response_body_limit(1024)
            .build()
            .unwrap();
        let error = limited.payload().await.unwrap_err();
        assert!(error.is_protocol());
        assert!(error.to_string().contains("exceeds the configured limit"));
        server.abort();
    }
}
