use macroni::api;

#[api]
trait MissingExtensionParameter {
    #[get("/value")]
    #[extension(auth)]
    async fn value(&self) -> macroni::Result<()>;
}

fn main() {}

