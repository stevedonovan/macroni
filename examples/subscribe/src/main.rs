use std::ffi::OsString;
// Run with: cargo run -p subscribe
//
// Then in another terminal:
// curl -N 'http://localhost:1212/subscribe?name=/my/path'
use inotify::{EventMask, Inotify, WatchMask};
use macroni::axum::http::Method;
use macroni::serde::{Deserialize, Serialize};
use macroni::{EventStream, Result, StreamExt};
use macroni::{StatusCode, tokio};
use tower_http::cors;
use tower_http::cors::CorsLayer;
use tracing::instrument;
use tracing_subscriber::fmt::format::FmtSpan;

struct Source {}

impl Source {
    fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct IEvent {
    pub mask: String,
    pub cookie: u32,
    pub name: Option<String>,
}

fn mask_to_string(mask: EventMask) -> String {
    let s = format!("{:?}", mask);
    if let Some(idx) = s.find("(") {
        s[(idx + 1)..(s.len() - 1)].to_string()
    } else {
        s
    }
}

impl IEvent {
    fn new(e: inotify::Event<OsString>) -> Self {
        Self {
            mask: mask_to_string(e.mask),
            cookie: e.cookie,
            name: e.name.map(|x| x.to_string_lossy().into_owned()),
        }
    }
}

fn err(s: &str) -> macroni::Error {
    macroni::Error::user(StatusCode::BAD_REQUEST, s, "bad request")
}

#[macroni::api]
impl Source {
    #[get("/errors")]
    #[sse]
    #[instrument(skip(self))]
    async fn errors(&self) -> Result<EventStream<String>> {
        let stream = tokio_stream::iter([Ok("one".into()), Err(err("two")), Ok("three".into())]);
        Ok(EventStream::new(stream))
    }

    #[get("/subscribe")]
    #[sse]
    async fn subscribe(&self, path: String) -> Result<EventStream<IEvent>> {
        let inotify = Inotify::init().unwrap();
        inotify
            .watches()
            .add(
                &path,
                WatchMask::MODIFY | WatchMask::CREATE | WatchMask::DELETE,
            )
            .map_err(macroni::Error::transport)?;

        let stream = inotify
            .into_event_stream([0u8; 1024])
            .map_err(macroni::Error::transport)?
            .map(|event| event.map(IEvent::new).map_err(macroni::Error::transport));
        Ok(EventStream::new(stream))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(FmtSpan::ENTER | FmtSpan::EXIT)
        .init();

    tracing::debug!("hello here");

    tracing::debug_span!("and here we go");

    let cors = CorsLayer::new()
        .allow_origin(cors::Any)
        .allow_methods([Method::GET]);

    let listener = SourceServer::router(Source::new()).layer(cors);

    macroni::serve!("0.0.0.0:1212", listener);
    Ok(())
}
