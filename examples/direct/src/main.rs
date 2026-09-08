use macroni::serde_json::Value;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Args {
    name: String,
    age: u32,
}

#[macroni::api(client)]
pub trait ExternalApi {
    #[get("/hello")]
    async fn hello(&self, name: String, age: u32) -> macroni::Result<String>;

    #[post("/hello")]
    #[body(args)]
    async fn hello_post(&self, args: Args) -> macroni::Result<Value>;

    #[post("/set/{id}")]
    async fn set_id(&self, id: String) -> macroni::Result<()>;
}

#[tokio::main]
async fn main() {
    let address = std::env::var("HELLO_ADDR").unwrap_or_else(|_| "http://127.0.0.1:3000".into());
    let client = ExternalApiClient::new(address).expect("client");
    let res = client.hello("John Doe".into(), 24).await.unwrap();
    println!("Hello, {res}!");
    client.set_id(String::from("Hello!")).await.unwrap();
    let res = client
        .hello_post(Args {
            name: "hello".to_string(),
            age: 10,
        })
        .await
        .unwrap();
    println!("goodbye {res}");
}
