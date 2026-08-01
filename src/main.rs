use axum::extract::{FromRequestParts, Path, Query};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(hello).post(up))
        .route("/mine", post(with_body))
        .route("/body/{name}/{age}", get(with_path))
        .route("/query", get(with_query));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Listening on http://localhost:3000");
    axum::serve(listener, app).await.unwrap();
}

#[derive(Debug, Deserialize, Serialize)]
struct OurQuery {
    name: String,
    age: u32,
}

#[derive(Debug)]
enum MyError {
    BadQuery(String),
}

impl IntoResponse for MyError {
    fn into_response(self) -> Response {
        match self {
            MyError::BadQuery(s) => {
                (StatusCode::BAD_REQUEST, Json(json!({"error": s}))).into_response()
            }
        }
    }
}
impl<E> From<E> for MyError
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(e: E) -> Self {
        MyError::BadQuery(e.to_string())
    }
}

async fn hello() -> Json<Value> {
    Json(json!( {"message": "hello, world, dudes"}))
}

async fn up(msg: String) -> StatusCode {
    println!("received message: {}", msg);
    StatusCode::OK
}

async fn with_body(Json(q): Json<OurQuery>) -> Json<Value> {
    println!("received query: {q:?}");
    Json(json!({ "message": q.name}))
}
struct MyPath<T>(pub T);

impl<S, T> FromRequestParts<S> for MyPath<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = MyError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(val)) => Ok(MyPath(val)),
            Err(err) => Err(MyError::BadQuery(err.to_string())),
        }
    }
}
/*
async fn with_path(q: Result<Path<OurQuery>, PathRejection>) -> Result<Json<Value>, MyError> {
    println!("received query: {q:?}");
    match q {
        Ok(q) => Ok(Json(serde_json::to_value::<OurQuery>(q.0)?)),
        Err(err) => Err(match err {
            PathRejection::FailedToDeserializePathParams(err) => MyError::BadQuery(err.to_string()),
            PathRejection::MissingPathParams(err) => MyError::BadQuery(err.to_string()),
            _ => MyError::BadQuery(err.to_string()),
        }),
    }
}

 */

async fn with_path(MyPath(q): MyPath<OurQuery>) -> Result<Json<Value>, MyError> {
    Ok(Json(json!({ "message": q.name })))
}

async fn with_query(Query(q): Query<OurQuery>) -> StatusCode {
    println!("received query: {q:?}");
    StatusCode::OK
}
