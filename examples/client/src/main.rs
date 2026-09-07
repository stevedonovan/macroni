use role_api::{Role, RoleApi, RoleApiClient};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address = std::env::var("ROLE_API_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let client = RoleApiClient::builder(format!("http://{address}"))?
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(1))
        .bearer_token("example-secret")?
        .build()?;
    let role = Role {
        name: "Administrator".into(),
        enabled: true,
    };

    client.post_by_name("admin".into(), role).await?;
    let stored = client.get_by_name("admin".into(), false).await?;
    println!("stored role: {stored:?}");
    println!("authenticated as: {}", client.who_am_i().await?);
    println!(
        "deleted role: {:?}",
        client.delete_by_name("admin".into(), true).await?
    );

    loop {
        match client.slow("frodo".into()).await {
            Ok(val) => {
                println!("we got: {:?}", val);
                if val == "finis" {
                    break;
                }
            }
            Err(e) => {
                if e.is_request_timeout() {
                    println!("timeout");
                } else {
                    return Err(e.into());
                }
            }
        }
    }

    Ok(())
}
