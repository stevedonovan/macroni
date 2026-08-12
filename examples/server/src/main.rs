use axum::response::IntoResponse;
use role_api::{AuthenticatedUser, Error, Result, Role, RoleApi, RoleApiServer};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

#[derive(Default)]
struct RoleService {
    roles: RwLock<HashMap<String, Role>>,
}

impl RoleApi for RoleService {
    async fn get_by_name(&self, name: String, include_disabled: bool) -> Result<Role> {
        let role = self
            .roles
            .read()
            .expect("role lock poisoned")
            .get(&name)
            .cloned()
            .ok_or_else(|| Error::not_found("role_not_found", "Role was not found"))?;

        if role.enabled || include_disabled {
            Ok(role)
        } else {
            Err(Error::not_found("role_not_found", "Role was not found"))
        }
    }

    async fn post_by_name(&self, name: String, role: Role) -> Result<()> {
        self.roles
            .write()
            .expect("role lock poisoned")
            .insert(name, role);
        Ok(())
    }

    async fn delete_by_name(&self, name: String, confirmed: bool) -> Result<Role> {
        if !confirmed {
            return Err(Error::bad_request(
                "confirmation_required",
                "Deletion must be confirmed",
            ));
        }
        self.roles
            .write()
            .expect("role lock poisoned")
            .remove(&name)
            .ok_or_else(|| Error::not_found("role_not_found", "Role was not found"))
    }

    async fn who_am_i(&self, auth: AuthenticatedUser) -> Result<String> {
        Ok(auth.user_id)
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "role_server=info,tower_http=info".into()),
        )
        .init();

    let app = RoleApiServer::router(Arc::new(RoleService::default()))
        .layer(macroni::server::body_limit(1024 * 1024))
        .layer(axum::middleware::from_fn_with_state(
            Duration::from_secs(10),
            macroni::server::timeout,
        ))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(axum::middleware::from_fn(authenticate));
    let address = std::env::var("ROLE_API_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .expect("bind server listener");

    println!("role server listening on http://{address}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("serve role API");
}

async fn authenticate(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let authorized = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some("Bearer example-secret");

    if !authorized {
        return Error::user(
            axum::http::StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid bearer token is required",
        )
        .into_response();
    }

    request.extensions_mut().insert(AuthenticatedUser {
        user_id: "example-user".into(),
    });
    next.run(request).await
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("install Ctrl-C handler");
}
