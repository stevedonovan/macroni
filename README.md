# A Simplified, more Ergonomic Way to use Axum

## Some Macro Magic

[Axum](https://docs.rs/axum) is a powerful and flexible Web framework by the Tokio team built
on the lower-level [Hyper](https://docs.rs/hyper) library that can use middleware from
the [Tower](https://docs.rs/tower) project.

However, a non-trivial `Axum` project requires assembling features from different crates, and
doing some customization, like making your error type support the `IntoResponse` trait, etc.

`macroni` aims to simplify the Axum developer experience for the common case of constructing REST-like JSON APIs.
It provides proc macro sugar for generating `Axum` handlers from trait methods or implementation blocks,
and sets up `Axum` extractors for you. It provides a suitable error type, and
implements custom extractors that use that type, so that errors are formatted as JSON.

However, the resulting generated routers are from `Axum` and
can use the whole ecosystem:

```rust
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
```

It looks very much like
a [minimal Axum example](https://github.com/tokio-rs/axum/blob/main/examples/hello-world/src/main.rs), except
instead of handlers as free async functions using extractor patterns, the handlers are generated from ordinary async
methods and the
arguments of a shared type (like query in case of GET and body in case of POST) are collected into generated structs.
The routes themselves
are specified as macro attributes, as with the `Rocket` framework.

Some conventions are followed when generating the actual `Axum` handlers. For GET handlers, if a
parameter is not explicitly from the path, then it comes from the query. Similarly, for a POST handler,
if not a path parameter, then it is assumed to be part of the JSON body. So the first method would be evoked as GET
`http://localhost:3030/hello?name=Bilbo&age=111` and the POST route will want a body like `{"name":"Bilbo","age":111}`

## Client _and_ Server Specified as a Trait

The original case I was envisaging was a Rust server, and a Rust client. In this case the macro `api` operates on an
_async trait_:

```rust
#[macroni::api]
pub trait MyApi {
    #[get("/role/{name}")]
    async fn get_by_name(&self, name: String) -> macroni::Result<Role>;

    #[post("/role/{name}")]
    async fn post_by_name(&self, name: String, role: Role) -> macroni::Result<()>;
}
```

When this trait is imported by a client Rust project, then the proc macro will generate a _client implementation_
using `Reqwest`.

The server side provides an implementation of the trait, that looks like this:

```rust
struct MyState {
    roles: Mutex<HashMap<String, Role>>
}

impl MyApi for MyState {
    async fn get_by_name(&self, name: String) -> macroni::Result<Role> {
        let roles = self.roles.lock().unwrap();
        ...
    }

    async fn post_by_name(&self, name: String, role: Role) -> macroni::Result<()> {
        ...
    }
}
```

The generated router stores the implementation in an `Arc<T>`. In effect, the handlers passed to
Axum look like this:

```rust
async fn get_by_name(State(this): State<Arc<MyState>>, Path(name): Path<String>) -> Result<Json<Role>> {}

async fn post_by_name(State(this): State<Arc<MyState>>, Json(role): Json<Role>) -> Result<Json<()>> {}
```

Note how the implementing struct automatically becomes state for all these routes.

Generated clients validate their base URL and use finite defaults: a two-second connection timeout,
a ten-second total request timeout, and an eight-MiB response limit. All can be configured:

```rust
let client = MyApiClient::builder("http://localhost:3000") ?
.connect_timeout(Duration::from_secs(1))
.timeout(Duration::from_secs(5))
.response_body_limit(2 * 1024 * 1024)
.default_headers(headers)
.build() ?;
let role = client.get_by_name("admin".into()) ?;
```

`with_http_client` remains available when the application wants complete control over `Reqwest`.

## Client and server features

`Axum` and `Reqwest` are optional dependencies, depending on what features you activate on `macroni`.
For example the `role-server` example's `Cargo.toml` is:

```toml
[package]
name = "role-server"
version = "0.1.0"
edition = "2024"

[dependencies]
axum = "0.8"
macroni = { path = "../..", default-features = false, features = ["server"] }
role-api = { path = "../api", features = ["server"] }
tokio = { version = "1.0", features = ["macros", "net", "rt-multi-thread", "signal"] }
tower-http = { version = "0.6", features = ["request-id", "trace"] }
tracing-subscriber = { version = "0.3", features = ["env-filter"] }```
```

Note how the shared trait is in a shared crate `role-api`. (The last two dependencies are optional)

```rust
#[macroni::api(client_feature = "client", server_feature = "server")]
pub trait RoleApi {
    // ...
}
```

A client enables only the contract's `client` feature, while a server enables only `server`.
Using `#[macroni::api]` without feature names generates both client and server unconditionally, except
in the case where `api` is applied to an `impl` block directly, which is always just server.

The server example deliberately composes the generated router with ordinary `Axum` and `Tower`
middleware. It demonstrates structured request tracing, request IDs, a JSON-producing timeout,
a request body limit, and graceful Ctrl-C shutdown. `macroni` does not install a tracing subscriber,
choose an authentication scheme, or hide the Axum `Router`.

`MyApiClient` directly implements `MyApi`. The crate's `Result` uses an error type that implements
`IntoResponse`. Expected user errors preserve their 4xx status and JSON error envelope; unexpected
internal errors are logged and returned as a redacted JSON 500 response. Extractor rejections are
also converted to JSON errors while preserving Axum's rejection status.

On the client, errors distinguish remote API responses, local transport failures, timeouts, and
protocol failures such as malformed JSON or an incorrect response content type. Non-success status
codes are preserved even when the remote error envelope is malformed.

## Other Method Attributes

A method can have a `#[extension(name)]` attribute, where `name` is one of the parameters.
`name: Type` is transformed into the Axum `Extension(name): Extension<Type>` when middleware has set a
typed extension value.

It is of course completely possible to generate _just_ a client implementation from a trait - see the
`direct` example. If we are matching an external API then it's necessary to match the shape of any JSON
bodies. For example, `fn hello_post(&self, arg: Args)` will by default generate a wrapper struct around
the single `arg` - at the code generation point, we really don't know if `Arg` is a struct not
requiring wrapping. The attribute `#[body(arg)]` indicates that we don't want a wrapper.
