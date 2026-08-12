use macroni::api;

#[api]
trait GenericMethod {
    #[get("/value")]
    async fn value<T>(&self, value: T) -> macroni::Result<()>;
}

fn main() {}
