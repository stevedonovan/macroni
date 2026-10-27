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

However, the resulting generated routers are from `Axum` and you
can use the whole ecosystem with them:

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

    #[post("/set/{id}")]
    pub async fn set_id(&mut self, id: String) -> Result<()> {
        self.id = id;
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let server = ImplementationServer::router(Implementation {
        id: "Admin".to_owned(),
    }); // this is an Axum router and you can add layers, middleware, etc

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
methods. The arguments of a shared type (like query in case of GET and body in case of POST) are collected into
generated structs.
The routes themselves are specified as macro attributes, as with the `Rocket` framework.

Note how in this case `&self` implicitly becomes `Arc<RwLock<Implementation>>` - if all the methods are immutable,
`&self`
is just `Arc<Implementation>`

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

(You *could* declare `post_by_name` with a `&mut self` receiver, and then the `Mutex` is unneeded.)

Generated clients validate their base URL and use finite defaults: a two-second connection timeout,
a ten-second total request timeout, and an eight-MiB response limit. All can be configured:

```rust,ignore
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
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

Note how the shared trait is in a shared crate `role-api`. (The last two dependencies are optional)

```rust
#[macroni::api]
pub trait RoleApi {
    // ...
}
```

A client enables only the contract's `client` feature, while a server enables only `server`.

Without feature flags, using `#[api(client)]` creates only the client, and `#[api(client,server)]` implements both.
In the case where `api` is applied to an `impl` block directly, it is always just `server`.

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

## Composing generated clients

Use `clients!` to group APIs behind named fields, mirroring merged server routers:

```rust,ignore
use my_api::{Hello, HelloClient, User, UserClient};

macroni::clients! {
    pub struct AppClient {
        pub hello: HelloClient,
        pub user: UserClient,
    }
}

let client = AppClient::builder("http://localhost:3000") ?
.bearer_token(token) ?
.timeout(Duration::from_secs(5))
.build() ?;

client.hello.greet("Steve".into()).await?;
client.user.get_by_id(42).await?;
```

## Gzip response compression

Enable the optional `gzip` feature to compress responses from generated routers and
automatically decode them in generated clients:

```toml
macroni = { version = "0.3", features = ["gzip"] }
```

It also works with `default-features = false` and either `server` or `client`.
Shared API crates can forward it with `gzip = ["macroni/gzip"]`; no changes to
traits, methods, or macro attributes are needed.

Servers use Tower HTTP's compression layer to negotiate gzip with clients that
advertise support. Tower's default compression rules apply, so small responses
may remain uncompressed. Clients automatically advertise gzip support and decode
responses before JSON parsing. The configured response body limit applies to the
decompressed bytes. A client supplied through `with_http_client` retains its own
compression settings. Request bodies are not compressed by this feature.

Compression covers the generated router's routes. To cover additional routes or
responses produced by outer middleware, apply compression to the final application
router instead.

## MessagePack encoding

Enable `msgpack` on both the client and server to use MessagePack in place of JSON:

```toml
macroni = { version = "0.3", features = ["msgpack"] }
```

This switches request bodies, successful responses, and macroni error responses
to `application/msgpack`. Paths and query parameters keep their existing encoding;
Bodies use named fields
(MessagePack maps), including generated parameter structs and error envelopes.
Types continue to use Serde's `Serialize` and `Deserialize` traits.

The format is selected at build time for the macroni dependency, not negotiated
per request. The feature works with client-only and server-only builds and combines with
`gzip`. Shared API crates can forward it with `msgpack = ["macroni/msgpack"]`
(as in `examples/api`). No macro attributes or application methods need to change.
Externally supplied Reqwest clients work too: macroni handles body encoding and
decoding itself.

Under the hood, the main change is to replace our `Json` extractor with a `Payload` extractor,
which is either defined for JSON or `msgpack`. This makes it very straightforward to implement
other wire formats based on feature flags.

## Other Method Attributes

A method can have a `#[extension(name)]` attribute, where `name` is one of the parameters.
`name: Type` is transformed into the Axum `Extension(name): Extension<Type>` when middleware has set a
typed extension value.

It is of course completely possible to generate _just_ a client implementation from a trait - see the
`direct` example. If we are matching an external API then it's necessary to match the shape of any JSON
bodies. For example, `fn hello_post(&self, arg: Args)` will by default generate a wrapper struct around
the single `arg` - at the point of code generation, we really don't know if `Arg` is a struct not
requiring wrapping. The attribute `#[body(arg)]` indicates that we don't want a wrapper.

## Mutable Receivers

From 0.3.0 onwards, a method may have a `&mut self` receiver. When the impl or trait has at least one of
these, then the shared state changes. Instead of `Arc<T>` the state becomes `Arc<tokio::sync::RwLock<T>>` and
any method with a mutable receiver will get write access, otherwise read access.

Generated clients preserve the trait's receivers, so calling a mutable method therefore requires a mutable
client binding. For concurrent calls, clone the client into separate mutable handles:

```rust,ignore
let mut first = client.clone();
let mut second = client.clone();
tokio::try_join!(first.update(a), second.update(b))?;
```

Clones share the HTTP connection pool. A mutable client borrow only restricts that
local handle; the server's read/write lock controls access to the shared implementation
and is held for the duration of the method, including across `.await`.

## Server-sent events

Async *streams* are very common in Rust application code. `macroni` allows methods to return and *consume*
async streams, which is useful for pub/sub approaches to API design. These are implemented by the HTTP protocols
server-side events and request chunking, and can only be used with plain JSON text as transport.

Use `#[sse]` with a GET route to return an owned async stream:

```rust
#[macroni::api]
pub trait Updates {
    #[get("/updates")]
    #[sse]
    async fn subscribe(&self) -> macroni::Result<macroni::EventStream<u64>>;
}
```

The implementation must return `EventStream::new(stream)`, where `stream` implements
`Stream<Item = macroni::Result<T>> + Send + 'static`.

Consumers choose whether to continue after an error:

```rust,ignore
let mut events = client.subscribe()?;
while let Some(item) = events.next().await {
    match item {
        Ok(event) => handle(event),
        Err(error) => eprintln!("Subscription reported: {error}"),
    }
}
```

Using `item?` instead stops consumption on the first error. Broadcast
subscriptions report lag as an error and then resume with retained messages.

SSE event payloads always use JSON without gzip, even if e.g. `msgpack` is enabled.
Setup errors use ordinary HTTP errors in the selected wire format.
After setup, a source error becomes a named `macroni-error` event containing
a JSON object with `status` and `response` fields. Internal errors are redacted.
The client yields that error as an item and continues reading. The producer
ends the subscription by returning `None`; to report a fatal source error,
yield the error and then end.

Browser clients need an explicit listener for application errors if they want to keep subscribing:

```javascript
eventSource.addEventListener('macroni-error', (event) => {
    const {status, response} = JSON.parse(event.data);
    console.error(status, response.code, response.message);
    // Keep listening; call eventSource.close() only if you choose to stop.
});
```

There is no reconnection by *default* (but see later), event replay, or delivery guarantee;
it is entirely up to the application how to manage subscriptions.

```rust,ignore
let client = UpdatesClient::builder(address)?
    .timeout(Duration::from_secs(10)) // Overall deadline for ordinary calls only.
    .stream_timeout(
        macroni::StreamTimeout::new()
            .response_headers(Duration::from_secs(10)) // time until first headers received
            .idle_timeout(Duration::from_secs(60)), // keepalive
    )
    .build()?;
```

See [the runnable pub/sub example](examples/events/src/main.rs):
`cargo run -p events`.

For a standalone SSE server, run `cargo run -p subscribe` and connect with
`curl -N http://localhost:1212/subscribe`. See [the subscribe example](examples/subscribe/src/main.rs).

### Retrying streaming connections

Enable retries explicitly on a generated client's builder (or with
`ClientConfig::with_stream_retry`):

```rust,ignore
let client = UpdatesClient::builder(address)?
    .stream_retry(
        macroni::StreamRetry::new()
            .max_retries(3)
            .backoff(Duration::from_millis(200), Duration::from_secs(30)),
    )
    .build()?;
let mut events = client.subscribe().await?;
while let Some(item) = events.next().await {
    match item {
        Ok(event) => handle(event),
        Err(error) if error.is_stream_retry_exhausted() => {
            eprintln!("Subscription exhausted its retry budget: {error}");
            break;
        }
        Err(error) => eprintln!("Subscription reported: {error}"),
    }
}
```

The default policy allows three reconnection attempts over the entire
subscription, starting at 200ms and capped at 30 seconds.

Reconnection starts a new subscription. No event IDs are replayed and no
`Last-Event-ID` is sent automatically, so events may be missed or duplicated; as before, policy
is up to you; you may choose to include an event ID in your payload, and choose to pass it to
your subscription method.

## Streaming request bodies

Use `#[stream_body]` on a POST, PUT, or PATCH method whose sole body argument
is an owned `EventStream<T>`:

```rust,ignore
#[macroni::api]
pub trait Import {
    #[post("/import/{batch}")]
    #[stream_body]
    async fn import(
        &self,
        batch: String,
        items: macroni::EventStream<Record>,
    ) -> macroni::Result<ImportSummary>;
}
```

Path parameters and extensions can accompany the stream.
The handler consumes items on demand and returns an ordinary response.

The usual state-lock behavior applies for mutable methods, so be careful with a long-lived
input stream.

Uploads use `application/x-ndjson` and UTF-8 JSON even with `msgpack` enabled.
Every record ends in LF or CRLF; HTTP chunk boundaries are irrelevant. The JSON records
encode `Result<T>` like this (the "heartbeat" type just keeps the connection alive and is discarded.)

```
{ "type": "data", "data": { "name": "first" }}
{ "type": "error", "error": {"code": "source_busy","message": "An intermittent source error"}}
{ "type": "heartbeat" }
```

On the Rust client side, the call is like this:

```rust,ignore
        let items = futures_util::stream::iter([
            Ok(json!{"name":"first"}),
            Err(macroni::Error::bad_request(
                "source_busy",
                "An intermittent source error",
            )),
        ]);
        let result = client.import("demo".into(), EventStream::new(items)).await;
```

Can match the protocol directly using this generator function with NodeJS's `fetch` API:

```js
async function* records() {
    yield line({type: 'data', data: 'first'});

    await sleep(1000, undefined, {signal});
    yield line({type: 'heartbeat'});

    yield line({
        type: 'error',
        error: {
            code: 'source_busy',
            message: 'An intermittent source error',
        },
    });

    await sleep(1000, undefined, {signal});
    // Returning completes the request body.
}

```

When implementing the method, data yields `Ok`; source errors yield `Err` and allow further records.
Error records contain the existing `ErrorResponse` fields, without an HTTP
status; the receiving handler sees them as user errors with status 400.
Serialization failures abort the upload. Clean body completion ends the input.

Configuring the two ends:

```rust,ignore
let client = ImportClient::builder(address)?
    .stream_body(
        macroni::StreamBodyConfig::new()
            .max_record_bytes(1024 * 1024)
            .heartbeat_interval(Duration::from_secs(15))
            .overall_timeout(Duration::from_secs(300)),
    )
    .build()?;

let router = ImportServer::router(service).layer(macroni::axum::Extension(
    macroni::StreamBodyConfig::new()
        .max_record_bytes(1024 * 1024) // default one Mb record limit
        .idle_timeout(Duration::from_secs(60))
        .overall_timeout(Duration::from_secs(300)),
//));
```

Streaming uploads bypass the ordinary client's total request timeout.
The connect timeout still applies. After the upload finishes, the configured
`StreamTimeout::response_headers` bounds waiting for the response headers;
it also bounds reading the finite response body. A custom Reqwest client's
own deadlines still apply. The response uses the normal selected wire codec.

Streamed uploads are never automatically retried, even when SSE retries are
enabled. Processing acknowledgements, replay, source overflow policy, and
transactional handling of partial uploads belong to the application.

Run `cargo run -p upload` for [a complete example](examples/upload/src/main.rs).
