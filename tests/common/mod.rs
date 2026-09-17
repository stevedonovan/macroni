pub async fn decode<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> T {
    #[cfg(feature = "msgpack")]
    {
        assert_eq!(response.headers()["content-type"], "application/msgpack");
        rmp_serde::from_slice(&response.bytes().await.unwrap()).unwrap()
    }
    #[cfg(not(feature = "msgpack"))]
    {
        assert_eq!(response.headers()["content-type"], "application/json");
        response.json().await.unwrap()
    }
}
