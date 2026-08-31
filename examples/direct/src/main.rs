use macroni::serde_json::Value;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Args {
    name: String,
    age: u32,
}

#[macroni::api(client_feature = "client", server_feature = "server")]
pub trait ExternalApi {
    #[get("/hello")]
    async fn hello(&self, name: String, age: u32) -> macroni::Result<String>;

    #[post("/hello")]
    #[body(args)]
    async fn hello_post(&self, args: Args) -> macroni::Result<Value>;
}

#[tokio::main]
async fn main() {
    let client = ExternalApiClient::new("http://localhost:3000").expect("client");
    let res = client.hello("John Doe".into(), 24).await.unwrap();
    println!("Hello, {res}!");
    let res = client
        .hello_post(Args {
            name: "hello".to_string(),
            age: 10,
        })
        .await
        .unwrap();
    println!("goodbye {res}");
}
