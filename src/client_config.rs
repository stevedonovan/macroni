use crate::{Error, Result};
use reqwest::{
    Client, Url,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use std::{marker::PhantomData, time::Duration};

const DEFAULT_RESPONSE_BODY_LIMIT: usize = 8 * 1024 * 1024;

/// Configuration shared by generated clients, including their HTTP connection pool.
///
/// Cloning this value shares the underlying Reqwest client. Use [`Self::builder`]
/// for either an HTTP(S) base URL or an absolute Unix socket path.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    base_url: Url,
    http: Client,
    max_response_bytes: usize,
}

impl ClientConfig {
    /// Build with a two-second connect timeout, ten-second request timeout,
    /// and eight-MiB decoded response limit.
    pub fn new(address: impl AsRef<str>) -> Result<Self> {
        Self::builder(address)?.build()
    }

    pub fn builder(address: impl AsRef<str>) -> Result<ClientConfigBuilder> {
        ClientConfigBuilder::new(address)
    }

    /// Use an existing HTTP client without changing its settings.
    ///
    /// `base_url` must be HTTP(S). For a custom Unix socket client, configure
    /// the socket on the supplied Reqwest client and use `http://localhost` here.
    pub fn with_http_client(base_url: impl AsRef<str>, http: Client) -> Result<Self> {
        let (base_url, socket) = parse_url(base_url.as_ref())?;
        if socket.is_some() {
            return Err(Error::protocol(
                None,
                "with_http_client requires an HTTP(S) base URL; configure the Unix socket on the supplied HTTP client",
                None,
            ));
        }
        Ok(Self {
            base_url,
            http,
            max_response_bytes: DEFAULT_RESPONSE_BODY_LIMIT,
        })
    }

    /// The HTTP base URL (a placeholder host when using a Unix socket).
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// Construct a generated request beneath the base URL's path prefix.
    ///
    /// Route templates are validated by the macro. Parameter values are literal
    /// segments, never URL references.
    #[doc(hidden)]
    pub fn request_url(&self, template: &str, parameters: &[(&str, String)]) -> Result<Url> {
        let mut path = template.to_owned();
        for (placeholder, value) in parameters {
            if matches!(value.as_str(), "" | "." | "..") {
                return Err(Error::protocol(
                    None,
                    format!(
                        "invalid path parameter {placeholder}: empty and dot segments are unsupported"
                    ),
                    None,
                ));
            }
            path = path.replace(placeholder, &crate::__private::encode_path_segment(value));
        }
        let mut url = self.base_url.clone();
        let prefix = url.path().trim_end_matches('/');
        let path = format!("{prefix}{path}");
        url.set_path(&path);
        Ok(url)
    }

    /// The shared HTTP client and connection pool.
    pub fn http_client(&self) -> &Client {
        &self.http
    }

    /// Maximum decoded response body size in bytes.
    pub fn response_body_limit(&self) -> usize {
        self.max_response_bytes
    }

    /// Override the response limit without rebuilding the shared HTTP client.
    pub fn with_response_body_limit(mut self, max_bytes: usize) -> Self {
        self.max_response_bytes = max_bytes;
        self
    }
}

/// Builds shared configuration, or a generated client constructed from it.
///
/// Normally obtained through [`ClientConfig::builder`] or a generated client's
/// `builder` method. The latter selects its client type as `C` automatically.
#[derive(Debug)]
pub struct ClientConfigBuilder<C = ClientConfig> {
    base_url: Url,
    http: reqwest::ClientBuilder,
    default_headers: HeaderMap,
    max_response_bytes: usize,
    client: PhantomData<fn() -> C>,
}

impl<C> ClientConfigBuilder<C> {
    pub fn new(address: impl AsRef<str>) -> Result<Self> {
        let (base_url, socket) = parse_url(address.as_ref())?;
        let mut http = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(2));
        if let Some(socket) = socket {
            http = http.unix_socket(socket);
        }
        Ok(Self {
            base_url,
            http,
            default_headers: HeaderMap::new(),
            max_response_bytes: DEFAULT_RESPONSE_BODY_LIMIT,
            client: PhantomData,
        })
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.http = self.http.timeout(timeout);
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.http = self.http.connect_timeout(timeout);
        self
    }

    pub fn default_headers(mut self, headers: HeaderMap) -> Self {
        self.default_headers.extend(headers);
        self
    }

    pub fn authorization(mut self, mut value: HeaderValue) -> Self {
        value.set_sensitive(true);
        self.default_headers.insert(AUTHORIZATION, value);
        self
    }

    pub fn bearer_token(self, token: impl AsRef<str>) -> Result<Self> {
        let value =
            HeaderValue::from_str(&format!("Bearer {}", token.as_ref())).map_err(|error| {
                Error::protocol(None, format!("invalid bearer token: {error}"), None)
            })?;
        Ok(self.authorization(value))
    }

    pub fn response_body_limit(mut self, max_bytes: usize) -> Self {
        self.max_response_bytes = max_bytes;
        self
    }

    /// Build one HTTP client, then construct the requested target from its config.
    pub fn build(self) -> Result<C>
    where
        C: From<ClientConfig>,
    {
        let http = self
            .http
            .default_headers(self.default_headers)
            .build()
            .map_err(Error::from)?;
        Ok(C::from(ClientConfig {
            base_url: self.base_url,
            http,
            max_response_bytes: self.max_response_bytes,
        }))
    }
}

pub fn parse_url(address: &str) -> Result<(Url, Option<std::path::PathBuf>)> {
    if address.starts_with('/') {
        Ok(("http://localhost".parse().unwrap(), Some(address.into())))
    } else {
        let url = Url::parse(address).map_err(|error| {
            Error::protocol(None, format!("invalid API base URL: {error}"), None)
        })?;
        if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
            return Err(Error::protocol(
                None,
                "API base URL must be an absolute HTTP or HTTPS URL",
                None,
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(Error::protocol(
                None,
                "API base URL must not contain a query string or fragment",
                None,
            ));
        }
        Ok((url, None))
    }
}
