// Run with cargo run -p upload
use macroni::{EventStream, Result, StreamBodyConfig, StreamExt, axum, tokio};
use std::time::Duration;

#[macroni::api(client, server)]
pub trait Import {
    #[post("/import/{batch}")]
    #[stream_body]
    async fn import(&self, batch: String, items: EventStream<String>) -> Result<usize>;
}
struct Service;
impl Import for Service {
    async fn import(&self, batch: String, mut items: EventStream<String>) -> Result<usize> {
        let mut count = 0;
        while let Some(item) = items.next().await {
            match item {
                Ok(value) => {
                    println!("{batch}: {value}");
                    count += 1;
                }
                Err(error) if error.code() == Some("source_busy") => eprintln!("{error}"),
                Err(error) => return Err(error),
            }
        }
        Ok(count)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let listener = macroni::tcp_listener("0.0.0.0:4040")
        .await
        .map_err(macroni::Error::transport)?;
    let address = format!(
        "http://{}",
        listener.local_addr().map_err(macroni::Error::transport)?
    );
    let router = ImportServer::router(Service).layer(axum::Extension(
        StreamBodyConfig::new()
            .max_record_bytes(4096)
            .idle_timeout(Duration::from_secs(60)),
    ));
    if std::env::args().len() > 1 {
        axum::serve(listener, router).await.unwrap();
    } else {
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = ImportClient::builder(address)?
            .stream_body(
                StreamBodyConfig::new()
                    .heartbeat_interval(Duration::from_secs(15))
                    .overall_timeout(Duration::from_secs(30)),
            )
            .build()?;
        let items = futures_util::stream::iter([
            Ok("first".to_owned()),
            Err(macroni::Error::bad_request(
                "source_busy",
                "An intermittent source error",
            )),
            Ok("last".to_owned()),
        ]);
        let result = client.import("demo".into(), EventStream::new(items)).await;
        server.abort();
        println!("Imported {} records", result?);
    }
    Ok(())
}
