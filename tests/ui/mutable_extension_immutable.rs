pub struct Context;

#[macroni::api(client)]
pub trait Counter {
    #[post("/increment")]
    #[extension(context)]
    async fn increment(&mut self, context: Context) -> macroni::Result<()>;
}

async fn requires_mutable_binding(client: CounterClient) {
    client.increment().await.unwrap();
}

fn main() {}
