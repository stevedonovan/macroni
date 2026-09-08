use axum::response::IntoResponse;
use macroni::{Error, Result, api};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Role {
    name: String,
    enabled: bool,
}

#[derive(Clone, Debug)]
pub struct AuthenticatedUser {
    name: String,
}

#[api(server, client)]
pub trait RoleApi {
    #[get("/role/{name}")]
    async fn get_by_name(&self, name: String, include_disabled: bool) -> Result<Role>;

    #[post("/role/{name}")]
    async fn post_by_name(&self, name: String, role: Role) -> Result<()>;

    #[delete("/role/{name}")]
    async fn delete_by_name(&self, name: String, confirmed: bool) -> Result<Role>;

    #[put("/role/{name}")]
    async fn replace_by_name(&self, name: String, role: Role) -> Result<Role>;

    #[patch("/role/{name}")]
    async fn set_enabled(&self, name: String, enabled: bool) -> Result<Role>;

    #[get("/slow")]
    async fn slow(&self) -> Result<()>;

    #[get("/whoami")]
    #[extension(auth)]
    async fn who_am_i(&self, auth: AuthenticatedUser) -> Result<String>;
}

#[derive(Default)]
struct RoleService {
    roles: RwLock<HashMap<String, Role>>,
}

async fn authenticate(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if request
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some("Bearer test-secret")
    {
        return Error::user(
            http::StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Bearer token required",
        )
        .into_response();
    }
    request.extensions_mut().insert(AuthenticatedUser {
        name: "test-user".into(),
    });
    next.run(request).await
}

#[test]
fn generated_client_validates_its_base_url() {
    let error = RoleApiClient::new("not a URL").expect_err("invalid URL should fail");
    assert!(error.is_protocol());
    assert!(error.to_string().contains("invalid API base URL"));
}

impl RoleApi for RoleService {
    async fn get_by_name(&self, name: String, include_disabled: bool) -> Result<Role> {
        let role = self
            .roles
            .read()
            .expect("role lock poisoned")
            .get(&name)
            .cloned()
            .ok_or_else(|| {
                Error::not_found("role_not_found", format!("Role {name} was not found"))
            })?;

        if role.enabled || include_disabled {
            Ok(role)
        } else {
            Err(Error::not_found(
                "role_not_found",
                format!("Role {name} was not found"),
            ))
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

    async fn replace_by_name(&self, name: String, role: Role) -> Result<Role> {
        self.roles
            .write()
            .expect("role lock poisoned")
            .insert(name, role.clone());
        Ok(role)
    }

    async fn set_enabled(&self, name: String, enabled: bool) -> Result<Role> {
        let mut roles = self.roles.write().expect("role lock poisoned");
        let role = roles
            .get_mut(&name)
            .ok_or_else(|| Error::not_found("role_not_found", "Role was not found"))?;
        role.enabled = enabled;
        Ok(role.clone())
    }

    async fn slow(&self) -> Result<()> {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        Ok(())
    }

    async fn who_am_i(&self, auth: AuthenticatedUser) -> Result<String> {
        Ok(auth.name)
    }
}

#[tokio::test]
async fn generated_client_and_router_round_trip() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let address = listener.local_addr().expect("test listener address");
    let router = RoleApiServer::router(RoleService::default())
        .layer(macroni::server::body_limit(1024))
        .layer(axum::middleware::from_fn_with_state(
            std::time::Duration::from_millis(10),
            macroni::server::timeout,
        ))
        .layer(axum::middleware::from_fn(authenticate));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve test API");
    });

    let unauthorized =
        RoleApiClient::new(format!("http://{address}")).expect("build unauthorized API client");
    let error = unauthorized
        .who_am_i()
        .await
        .expect_err("request without bearer token should fail");
    assert_eq!(error.status(), Some(http::StatusCode::UNAUTHORIZED));

    let client = RoleApiClient::builder(format!("http://{address}"))
        .expect("build API client")
        .bearer_token("test-secret")
        .expect("valid bearer token")
        .build()
        .expect("build authenticated API client");
    let role = Role {
        name: "Administrator".into(),
        enabled: true,
    };

    client
        .post_by_name("admin user".into(), role.clone())
        .await
        .expect("post role");
    assert_eq!(
        client
            .get_by_name("admin user".into(), false)
            .await
            .expect("get role"),
        role
    );

    let error = client
        .get_by_name("missing".into(), false)
        .await
        .expect_err("missing role should fail");
    assert_eq!(error.status(), Some(http::StatusCode::NOT_FOUND));
    assert!(error.to_string().contains("role_not_found"));
    assert!(error.is_server());
    assert_eq!(error.code(), Some("role_not_found"));

    let timeout = client
        .slow()
        .await
        .expect_err("slow request should time out");
    assert_eq!(timeout.status(), Some(http::StatusCode::GATEWAY_TIMEOUT));
    assert_eq!(timeout.code(), Some("request_timeout"));
    assert_eq!(
        client.who_am_i().await.expect("read auth extension"),
        "test-user"
    );
    let replacement = Role {
        name: "Site Administrator".into(),
        enabled: false,
    };
    assert_eq!(
        client
            .replace_by_name("admin user".into(), replacement.clone())
            .await
            .expect("replace role"),
        replacement
    );
    let enabled_replacement = Role {
        enabled: true,
        ..replacement
    };
    assert_eq!(
        client
            .set_enabled("admin user".into(), true)
            .await
            .expect("patch role"),
        enabled_replacement
    );
    assert_eq!(
        client
            .delete_by_name("admin user".into(), true)
            .await
            .expect("delete role"),
        enabled_replacement
    );

    let malformed = reqwest::Client::new()
        .post(format!("http://{address}/role/admin"))
        .header("authorization", "Bearer test-secret")
        .header("content-type", "application/json")
        .body("not JSON")
        .send()
        .await
        .expect("send malformed request");
    assert_eq!(malformed.status(), reqwest::StatusCode::BAD_REQUEST);
    let malformed_error = malformed
        .json::<macroni::ErrorResponse>()
        .await
        .expect("extractor rejection should be JSON");
    assert_eq!(malformed_error.code, "invalid_json");

    server.abort();
}
