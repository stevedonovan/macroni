#![cfg(feature = "client")]

use macroni::ClientConfig;

pub mod contracts {
    #[macroni::api]
    pub trait Hello {
        #[get("/hello")]
        async fn greet(&self, name: String) -> macroni::Result<String>;
    }

    #[macroni::api]
    pub trait User {
        #[get("/users/{id}")]
        async fn get_by_id(&self, id: u32) -> macroni::Result<String>;
    }
}

mod composed {
    macroni::clients! {
        pub struct AppClient {
            pub hello: super::contracts::HelloClient,
            pub user: super::contracts::UserClient,
        }
    }
}

use composed::AppClient;

#[test]
fn constructors_validate_addresses_and_headers() {
    for address in ["not a URL", "ftp://localhost", "file:///tmp/socket"] {
        assert!(ClientConfig::new(address).unwrap_err().is_protocol());
        assert!(AppClient::new(address).unwrap_err().is_protocol());
        assert!(
            contracts::HelloClient::with_http_client(address, reqwest::Client::new())
                .unwrap_err()
                .is_protocol()
        );
    }
    assert!(
        ClientConfig::builder("http://localhost")
            .unwrap()
            .bearer_token("invalid\nheader")
            .unwrap_err()
            .is_protocol()
    );
    assert!(ClientConfig::with_http_client("/tmp/socket", reqwest::Client::new()).is_err());

    // The existing generated builder type remains usable by name.
    let builder: contracts::HelloClientBuilder =
        contracts::HelloClient::builder("http://localhost").unwrap();
    let _: contracts::HelloClient = builder.build().unwrap();
    let config = ClientConfig::new("/tmp/macroni-composition-unused.sock").unwrap();
    let _: AppClient = config.clone().into();
    let _: contracts::HelloClient = config.into();
}

#[cfg(feature = "server")]
mod server_tests {
    use super::*;
    use axum::{
        extract::{ConnectInfo, Request},
        middleware::{self, Next},
        response::IntoResponse,
    };
    use contracts::{Hello, HelloServer, User, UserServer};
    use macroni::Result;
    use std::{
        collections::HashSet,
        net::SocketAddr,
        sync::{Arc, Mutex},
        time::Duration,
    };

    struct Service;

    impl Hello for Service {
        async fn greet(&self, name: String) -> Result<String> {
            if name == "slow" {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Ok(format!("Hello, {name}"))
        }
    }

    impl User for Service {
        async fn get_by_id(&self, id: u32) -> Result<String> {
            if id == 0 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Ok(format!("user-{id}"))
        }
    }

    fn headers() -> http::HeaderMap {
        let mut headers = http::HeaderMap::new();
        headers.insert("x-client-test", http::HeaderValue::from_static("shared"));
        headers
    }

    fn config(address: &str) -> ClientConfig {
        ClientConfig::builder(address)
            .unwrap()
            .default_headers(headers())
            .bearer_token("test-secret")
            .unwrap()
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap()
    }

    fn router(peers: Arc<Mutex<HashSet<SocketAddr>>>) -> axum::Router {
        HelloServer::router(Service)
            .merge(UserServer::router(Service))
            .layer(middleware::from_fn(move |request: Request, next: Next| {
                let peers = peers.clone();
                async move {
                    if request
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        != Some("Bearer test-secret")
                        || request
                            .headers()
                            .get("x-client-test")
                            .and_then(|v| v.to_str().ok())
                            != Some("shared")
                    {
                        return macroni::Error::user(
                            http::StatusCode::UNAUTHORIZED,
                            "unauthorized",
                            "missing shared headers",
                        )
                        .into_response();
                    }
                    if let Some(ConnectInfo(peer)) =
                        request.extensions().get::<ConnectInfo<SocketAddr>>()
                    {
                        peers.lock().unwrap().insert(*peer);
                    }
                    next.run(request).await
                }
            }))
    }

    async fn exercise(address: &str, peers: &Mutex<HashSet<SocketAddr>>) {
        let shared = config(address);
        let app = AppClient::from_config(shared.clone());
        assert_eq!(
            app.hello.greet("Steve".into()).await.unwrap(),
            "Hello, Steve"
        );
        assert_eq!(app.user.get_by_id(42).await.unwrap(), "user-42");
        let clone = app.clone();
        assert_eq!(clone.user.get_by_id(7).await.unwrap(), "user-7");
        let standalone = contracts::HelloClient::from_config(shared.clone());
        assert_eq!(
            standalone.greet("again".into()).await.unwrap(),
            "Hello, again"
        );
        if address.starts_with("http") {
            assert_eq!(
                peers.lock().unwrap().len(),
                1,
                "aggregate fields and clones should share one TCP connection"
            );
        }

        let limited = AppClient::from_config(shared.with_response_body_limit(2));
        assert!(
            limited
                .hello
                .greet("Steve".into())
                .await
                .unwrap_err()
                .is_protocol()
        );
        assert!(limited.user.get_by_id(42).await.unwrap_err().is_protocol());

        let timed = AppClient::builder(address)
            .unwrap()
            .default_headers(headers())
            .bearer_token("test-secret")
            .unwrap()
            .timeout(Duration::from_millis(20))
            .build()
            .unwrap();
        assert!(
            timed
                .hello
                .greet("slow".into())
                .await
                .unwrap_err()
                .is_timeout()
        );
        assert!(timed.user.get_by_id(0).await.unwrap_err().is_timeout());

        let defaults = AppClient::new(address).unwrap();
        assert_eq!(
            defaults
                .hello
                .greet("Steve".into())
                .await
                .unwrap_err()
                .status(),
            Some(http::StatusCode::UNAUTHORIZED)
        );
    }

    #[tokio::test]
    async fn aggregate_shares_config_and_connections_over_tcp() {
        let peers = Arc::new(Mutex::new(HashSet::new()));
        let listener = macroni::tcp_listener("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let router = router(peers.clone());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        exercise(&address, &peers).await;

        let mut custom_headers = headers();
        custom_headers.insert(
            "authorization",
            http::HeaderValue::from_static("Bearer test-secret"),
        );
        let http = reqwest::Client::builder()
            .default_headers(custom_headers)
            .build()
            .unwrap();
        let app = AppClient::from_config(ClientConfig::with_http_client(&address, http).unwrap());
        assert_eq!(app.user.get_by_id(1).await.unwrap(), "user-1");
        assert_eq!(
            app.hello.greet("custom".into()).await.unwrap(),
            "Hello, custom"
        );
        server.abort();
    }

    #[tokio::test]
    async fn aggregate_shares_config_over_unix_socket() {
        struct SocketDir(std::path::PathBuf);
        impl Drop for SocketDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(self.0.join("api.sock"));
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = SocketDir(
            std::env::temp_dir().join(format!("macroni-test-{}-{unique:x}", std::process::id())),
        );
        std::fs::create_dir(&directory.0).unwrap();
        let address = directory.0.join("api.sock").to_str().unwrap().to_owned();
        let listener = macroni::unix_listener(&address).await.unwrap();
        let peers = Arc::new(Mutex::new(HashSet::new()));
        let router = router(peers.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        exercise(&address, &peers).await;
        server.abort();
        let _ = server.await;
    }
}
