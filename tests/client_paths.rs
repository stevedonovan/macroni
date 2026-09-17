#![cfg(feature = "client")]

#[macroni::api]
pub trait EchoPath {
    #[get("/echo/{value}")]
    async fn echo(&self, value: String) -> macroni::Result<String>;
}

#[test]
fn prefix_and_origin_are_preserved() {
    for base in ["https://example.com/api/v1", "https://example.com/api/v1/"] {
        let config = macroni::ClientConfig::new(base).unwrap();
        let url = config
            .request_url("/echo/{value}", &[("{value}", "a\\b/?#%".into())])
            .unwrap();
        assert_eq!(
            url.as_str(),
            "https://example.com/api/v1/echo/a%5Cb%2F%3F%23%25"
        );
        assert_eq!(config.request_url("/", &[]).unwrap().path(), "/api/v1/");
    }
    let config = macroni::ClientConfig::new("https://example.com/a%20b/").unwrap();
    assert_eq!(
        config.request_url("/echo", &[]).unwrap().path(),
        "/a%20b/echo"
    );
    for base in ["https://example.com/?x=1", "https://example.com/#anchor"] {
        assert!(macroni::ClientConfig::new(base).is_err());
    }
}

#[tokio::test]
async fn invalid_parameters_fail_before_connecting() {
    let client = EchoPathClient::new("http://127.0.0.1:1").unwrap();
    for value in ["", ".", ".."] {
        let error = client.echo(value.into()).await.unwrap_err();
        assert!(
            error.to_string().contains("invalid path parameter"),
            "{error}"
        );
    }
}

#[cfg(feature = "server")]
#[tokio::test]
async fn literal_parameters_round_trip_under_a_prefix() {
    struct Service;
    impl EchoPath for Service {
        async fn echo(&self, value: String) -> macroni::Result<String> {
            Ok(value)
        }
    }
    let listener = macroni::tcp_listener("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let router = axum::Router::new().nest("/api/v1", EchoPathServer::router(Service));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for suffix in ["/api/v1", "/api/v1/"] {
        let client = EchoPathClient::new(format!("{address}{suffix}")).unwrap();
        for value in [
            "hello world",
            "日本語",
            "a/b",
            "a\\b",
            "%2e%2e",
            "%",
            "a?#b",
            "{value}",
            "a..b",
            "../other",
            "//other",
        ] {
            assert_eq!(client.echo(value.into()).await.unwrap(), value);
        }
    }
    server.abort();
    let _ = server.await;
}
