use macroni::api;
use serde::{Deserialize, Serialize};

pub use macroni::{Error, Result};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Role {
    pub name: String,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct AuthenticatedUser {
    pub user_id: String,
}

#[api]
pub trait RoleApi {
    #[get("/role/{name}")]
    async fn get_by_name(&self, name: String, include_disabled: bool) -> Result<Role>;

    #[post("/role/{name}")]
    async fn post_by_name(&self, name: String, role: Role) -> Result<()>;

    #[delete("/role/{name}")]
    async fn delete_by_name(&self, name: String, confirmed: bool) -> Result<Role>;

    #[get("/whoami")]
    #[extension(auth)]
    async fn who_am_i(&self, auth: AuthenticatedUser) -> Result<String>;

    #[post("/sub")]
    async fn sub(&self, sub: String) -> Result<bool>;

    #[get("/slow/{id}")]
    async fn slow(&self, id: String) -> Result<String>;
}
