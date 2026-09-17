//! The wire format shared by generated clients and servers.

use serde::Serialize;
#[cfg(feature = "client")]
use serde::de::DeserializeOwned;

#[cfg(not(feature = "msgpack"))]
pub const CONTENT_TYPE: &str = "application/json";
#[cfg(feature = "msgpack")]
pub const CONTENT_TYPE: &str = "application/msgpack";

#[cfg(not(feature = "msgpack"))]
#[cfg(feature = "client")]
pub const FORMAT: &str = "JSON";
#[cfg(feature = "msgpack")]
#[cfg(feature = "client")]
pub const FORMAT: &str = "MessagePack";

pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, EncodeError> {
    #[cfg(not(feature = "msgpack"))]
    return serde_json::to_vec(value);
    #[cfg(feature = "msgpack")]
    return rmp_serde::to_vec_named(value);
}

#[cfg(not(feature = "msgpack"))]
type EncodeError = serde_json::Error;
#[cfg(feature = "msgpack")]
type EncodeError = rmp_serde::encode::Error;

#[cfg(feature = "client")]
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, DecodeError> {
    #[cfg(not(feature = "msgpack"))]
    return serde_json::from_slice(bytes);
    #[cfg(feature = "msgpack")]
    return rmp_serde::from_slice(bytes);
}

#[cfg(all(feature = "client", not(feature = "msgpack")))]
type DecodeError = serde_json::Error;
#[cfg(all(feature = "client", feature = "msgpack"))]
type DecodeError = rmp_serde::decode::Error;

#[cfg(feature = "client")]
pub fn matches_content_type(value: &str) -> bool {
    let media_type = value.split(';').next().unwrap_or_default().trim();
    let Some(subtype) = media_type.strip_prefix("application/") else {
        return false;
    };
    #[cfg(not(feature = "msgpack"))]
    return subtype == "json" || subtype.ends_with("+json");
    #[cfg(feature = "msgpack")]
    return matches!(subtype, "msgpack" | "x-msgpack") || subtype.ends_with("+msgpack");
}
