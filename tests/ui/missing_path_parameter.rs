use macroni::api;

#[api]
trait MissingPathParameter {
    #[get("/value/{name}")]
    async fn value(&self, id: String) -> macroni::Result<()>;
}

fn main() {}
