// Run with: cargo run -p events
use macroni::{EventStream, Result, StreamExt, StreamTimeout};
use macroni::{axum, tokio};
use std::time::Duration;
use tokio::sync::broadcast;

#[macroni::api(client, server)]
pub trait Updates {
    #[get("/updates")]
    #[sse]
    async fn subscribe(&self) -> Result<EventStream<u64>>;
}

// The router can create subscriptions without keeping the publisher alive.
struct Publisher(broadcast::WeakSender<u64>);

impl Updates for Publisher {
    async fn subscribe(&self) -> Result<EventStream<u64>> {
        // Each caller gets its own owned receiver.
        let sender = self.0.upgrade().ok_or_else(|| {
            macroni::Error::not_found("publisher_closed", "The publisher has finished")
        })?;
        Ok(EventStream::new_from_broadcast(&sender).heartbeat_interval(Duration::from_secs(15)))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let (sender, _) = broadcast::channel(16);
    let local = "127.0.0.1:7070";
    let listener = macroni::tcp_listener(local)
        .await
        .map_err(macroni::Error::transport)?;
    let address = format!("http://{local}");
    let router = UpdatesServer::router(Publisher(sender.downgrade()));

    // server in background
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    // and then we can send some values in the background
    let client = UpdatesClient::builder(address)?
        .stream_timeout(StreamTimeout::new().idle_timeout(Duration::from_secs(60)))
        .build()?;
    let mut events = client.subscribe().await?;
    let mut others = client.subscribe().await?;

    let publisher = tokio::spawn(async move {
        for value in 0..5 {
            sender.send(value).unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // Dropping the last strong sender closes all subscriptions after they drain.
    });

    tokio::spawn(async move {
        // receive the results of the other subscription
        while let Some(event) = others.next().await {
            println!("And: {}", event.unwrap());
        }
    });

    // and receive the results of the subscription
    while let Some(event) = events.next().await {
        println!("Update: {}", event?);
    }
    drop(events); // Cancel the subscription; no reconnection occurs.
    publisher.await.unwrap();
    server.abort();
    Ok(())
}
