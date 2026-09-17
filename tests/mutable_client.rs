#![cfg(feature = "client")]

#[derive(Clone)]
pub struct Context(pub u32);

#[macroni::api]
pub trait Counter {
    #[get("/value")]
    async fn value(&self) -> macroni::Result<u32>;

    #[post("/add")]
    async fn add(&mut self, amount: u32) -> macroni::Result<u32>;

    #[post("/add-with-context")]
    #[extension(context)]
    async fn add_with_context(&mut self, amount: u32, context: Context) -> macroni::Result<u32>;
}

#[test]
fn mutable_trait_client_builds_without_a_server() {
    fn implements_counter<T: Counter>(_: &T) {}
    let client = CounterClient::new("http://localhost").unwrap();
    implements_counter(&client);
}

#[cfg(feature = "server")]
#[tokio::test]
async fn cloned_mutable_clients_update_one_shared_implementation() {
    struct Service(u32);

    impl Counter for Service {
        async fn value(&self) -> macroni::Result<u32> {
            Ok(self.0)
        }

        async fn add(&mut self, amount: u32) -> macroni::Result<u32> {
            let previous = self.0;
            tokio::task::yield_now().await;
            self.0 = previous + amount;
            Ok(self.0)
        }

        async fn add_with_context(
            &mut self,
            amount: u32,
            context: Context,
        ) -> macroni::Result<u32> {
            self.add(amount * context.0).await
        }
    }

    let listener = macroni::tcp_listener("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let router = CounterServer::router(Service(0)).layer(axum::Extension(Context(3)));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut client = CounterClient::new(address).unwrap();
    assert_eq!(client.add(1).await.unwrap(), 1);
    assert_eq!(client.add_with_context(2).await.unwrap(), 7);
    assert_eq!(
        Counter::add_with_context(&mut client, 1, Context(999))
            .await
            .unwrap(),
        10
    );

    let mut first = client.clone();
    let mut second = client.clone();
    let (a, b) = tokio::try_join!(first.add(1), second.add(2)).unwrap();
    assert_ne!(a, b);
    assert_eq!(a.max(b), 13);
    let readonly = client.clone();
    assert_eq!(readonly.value().await.unwrap(), 13);
    server.abort();
    let _ = server.await;
}
