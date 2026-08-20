use macroni::Result;
use macroni::serde_json::{Value, json};
use std::sync::Arc;

struct Implementation {
    id: String,
}

#[macroni::api]
impl Implementation {
    #[get("/hello")]
    pub async fn hello(&self, name: String, age: u32) -> Result<String> {
        Ok(format!(
            "Hello {}, {} year old named {}",
            self.id, age, name
        ))
    }

    #[post("/hello")]
    pub async fn send_hello(&self, name: String, age: u32) -> Result<Value> {
        Ok(json!({"user": self.id, "name": name, "age": age}))
    }
}

#[tokio::main]
async fn main() {
    let server = ImplementationServer::router(Arc::new(Implementation {
        id: "Admin".to_owned(),
    }));

    let address = std::env::var("HELLO_ADDR").unwrap_or_else(|_| "127.0.0.1:3030".into());
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .expect("bind server listener");

    println!("role server listening on http://{address}");
    axum::serve(listener, server).await.unwrap();
}
