use macroni::api;

#[api]
trait InvalidReceiver {
    #[get("/value")]
    async fn value(&mut self) -> macroni::Result<()>;
}

fn main() {}
