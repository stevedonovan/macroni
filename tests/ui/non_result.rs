use macroni::api;

#[api]
trait NonResult {
    #[get("/value")]
    async fn value(&self) -> String;
}

fn main() {}

