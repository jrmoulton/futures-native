//! Platform-native HTTP client with reqwest-compatible API and Tower
//! integration
//!
//! This crate provides a reqwest-compatible HTTP client that uses
//! platform-native networking APIs for optimal performance and system
//! integration.

// Re-export common types
#[cfg(feature = "json")]
use serde::{Serialize, de::DeserializeOwned};
use {
    bytes::Bytes,
    http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri},
    http_body::{Body, Frame},
    http_body_util::Full,
    pin_project::pin_project,
    std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll, Waker},
        time::Duration,
    },
    tower::Service,
};
pub use {http, http_body, http_body_util, tower};

/// Main HTTP client that implements both reqwest-style API and Tower Service
#[derive(Clone, Debug)]
pub struct Client {
    inner: platform::PlatformClient,
    config: ClientConfig,
}

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub timeout: Option<Duration>,
    pub user_agent: Option<String>,
    pub default_headers: HeaderMap,
    pub redirect_policy: RedirectPolicy,
    pub cookie_store: bool,
    pub proxy: Option<String>,
    pub no_proxy: bool,
}

#[derive(Clone, Debug)]
pub enum RedirectPolicy {
    None,
    Limited(usize),
    Default,
}

impl Default for RedirectPolicy {
    fn default() -> Self {
        RedirectPolicy::Limited(10)
    }
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(30)),
            user_agent: Some("platform-http/1.0".to_string()),
            default_headers: HeaderMap::new(),
            redirect_policy: RedirectPolicy::default(),
            cookie_store: false,
            proxy: None,
            no_proxy: false,
        }
    }
}

impl Client {
    /// Create a new client with default configuration
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Create a client builder for configuration
    #[must_use]
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    // Reqwest-style convenience methods

    /// Convenience method to make a GET request to a URL
    pub fn get<U: Into<Uri>>(&self, url: U) -> RequestBuilder {
        self.request(Method::GET, url.into())
    }

    /// Convenience method to make a POST request to a URL
    pub fn post<U: Into<Uri>>(&self, url: U) -> RequestBuilder {
        self.request(Method::POST, url.into())
    }

    /// Convenience method to make a PUT request to a URL
    pub fn put<U: Into<Uri>>(&self, url: U) -> RequestBuilder {
        self.request(Method::PUT, url.into())
    }

    /// Convenience method to make a DELETE request to a URL
    pub fn delete<U: Into<Uri>>(&self, url: U) -> RequestBuilder {
        self.request(Method::DELETE, url.into())
    }

    /// Start building a request with the specified method and URL
    pub fn request<U: Into<Uri>>(&self, method: Method, url: U) -> RequestBuilder {
        RequestBuilder::new(self.clone(), method, url.into())
    }

    /// Execute a request directly (for Tower Service compatibility)
    pub async fn execute(&self, request: Request<Full<Bytes>>) -> Result<Response<ResponseBody>, Error> {
        let mut client = self.clone();
        client.call(request).await
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for configuring HTTP client (reqwest-compatible)
pub struct ClientBuilder {
    config: ClientConfig,
}

impl ClientBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ClientConfig::default(),
        }
    }

    /// Set request timeout
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = Some(timeout);
        self
    }

    /// Disable request timeout
    #[must_use]
    pub fn no_timeout(mut self) -> Self {
        self.config.timeout = None;
        self
    }

    /// Set user agent header
    #[must_use]
    pub fn user_agent<V: AsRef<str>>(mut self, value: V) -> Self {
        self.config.user_agent = Some(value.as_ref().to_string());
        self
    }

    /// Set default headers for all requests
    #[must_use]
    pub fn default_headers(mut self, headers: HeaderMap) -> Self {
        self.config.default_headers = headers;
        self
    }

    /// Enable automatic cookie storage and handling
    #[must_use]
    pub fn cookie_store(mut self, enable: bool) -> Self {
        self.config.cookie_store = enable;
        self
    }

    /// Set HTTP proxy
    #[must_use]
    pub fn proxy<S: AsRef<str>>(mut self, proxy_url: S) -> Self {
        self.config.proxy = Some(proxy_url.as_ref().to_string());
        self
    }

    /// Disable system proxy
    #[must_use]
    pub fn no_proxy(mut self) -> Self {
        self.config.no_proxy = true;
        self
    }

    /// Set redirect policy
    #[must_use]
    pub fn redirect(mut self, policy: RedirectPolicy) -> Self {
        self.config.redirect_policy = policy;
        self
    }

    /// Build the configured client
    #[must_use]
    pub fn build(self) -> Client {
        Client {
            inner: platform::PlatformClient::new(&self.config),
            config: self.config,
        }
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Request builder for constructing HTTP requests (reqwest-compatible)
#[derive(Debug)]
pub struct RequestBuilder {
    client: Client,
    request: Result<http::request::Builder, Error>,
    body: RequestBody,
}

#[derive(Debug)]
enum RequestBody {
    Empty,
    Bytes(Bytes),
    #[cfg(feature = "json")]
    Json(serde_json::Value),
    Form(Vec<(String, String)>),
}

impl RequestBuilder {
    fn new(client: Client, method: Method, url: Uri) -> Self {
        let request = Request::builder().method(method).uri(url);

        Self {
            client,
            request: Ok(request),
            body: RequestBody::Empty,
        }
    }

    /// Add a header to the request
    #[must_use]
    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        if let Ok(builder) = self.request {
            match HeaderValue::from_str(value.as_ref()) {
                Ok(header_value) => {
                    self.request = Ok(builder.header(key.as_ref(), header_value));
                }
                Err(e) => {
                    self.request = Err(Error::InvalidHeader(e));
                }
            }
        }
        self
    }

    /// Add multiple headers to the request
    #[must_use]
    pub fn headers(mut self, headers: HeaderMap) -> Self {
        if let Ok(mut builder) = self.request {
            for (name, value) in headers {
                if let Some(name) = name {
                    builder = builder.header(name, value);
                }
            }
            self.request = Ok(builder);
        }
        self
    }

    /// Set the request body from bytes
    #[must_use]
    pub fn body<B: Into<Bytes>>(mut self, body: B) -> Self {
        self.body = RequestBody::Bytes(body.into());
        self
    }

    /// Set the request body as form data
    #[must_use]
    #[cfg(feature = "json")]
    pub fn form<T>(mut self, form: &T) -> Self
    where
        T: Serialize,
    {
        // For simplicity, convert to Vec<(String, String)>
        // In a real implementation, you'd want proper form encoding
        match serde_json::to_value(form) {
            Ok(serde_json::Value::Object(map)) => {
                let pairs: Vec<(String, String)> = map
                    .into_iter()
                    .map(|(k, v)| {
                        if let serde_json::Value::String(s) = v {
                            (k, s)
                        } else {
                            (k, v.to_string())
                        }
                    })
                    .collect();
                self.body = RequestBody::Form(pairs);
                self = self.header("content-type", "application/x-www-form-urlencoded");
            }
            _ => {
                self.request = Err(Error::Platform("Failed to serialize form data".to_string()));
            }
        }
        self
    }

    /// Set the request body as JSON
    #[cfg(feature = "json")]
    #[must_use]
    pub fn json<T: Serialize>(mut self, json: &T) -> Self {
        match serde_json::to_value(json) {
            Ok(value) => {
                self.body = RequestBody::Json(value);
                self = self.header("content-type", "application/json");
            }
            Err(_) => {
                self.request = Err(Error::Platform("Failed to serialize JSON".to_string()));
            }
        }
        self
    }

    /// Send the request
    pub async fn send(self) -> Result<ResponseWrapper, Error> {
        let request_builder = self.request?;

        // Convert body to bytes
        let body_bytes = match self.body {
            RequestBody::Empty => Bytes::new(),
            RequestBody::Bytes(bytes) => bytes,
            #[cfg(feature = "json")]
            RequestBody::Json(value) => serde_json::to_vec(&value)
                .map_err(|_| Error::Platform("JSON serialization failed".to_string()))?
                .into(),
            RequestBody::Form(pairs) => {
                todo!()
            }
        };

        let request = request_builder.body(Full::new(body_bytes)).map_err(Error::Http)?;

        let response = self.client.execute(request).await?;
        Ok(ResponseWrapper::new(response))
    }
}

/// Wrapper for HTTP response that provides reqwest-compatible methods
#[derive(Debug)]
pub struct ResponseWrapper {
    inner: Response<ResponseBody>,
}

impl ResponseWrapper {
    fn new(response: Response<ResponseBody>) -> Self {
        Self { inner: response }
    }

    /// Get the status code
    pub fn status(&self) -> StatusCode {
        self.inner.status()
    }

    /// Get the headers
    pub fn headers(&self) -> &HeaderMap {
        self.inner.headers()
    }

    /// Get the response URL (not implemented in this basic version)
    pub fn url(&self) -> &str {
        // Would need to track the final URL after redirects
        ""
    }

    /// Check if the response status indicates success
    pub fn is_success(&self) -> bool {
        self.inner.status().is_success()
    }

    /// Get the response body as text
    pub async fn text(self) -> Result<String, Error> {
        let bytes = self.bytes().await?;
        String::from_utf8(bytes.to_vec()).map_err(|_| Error::Platform("Invalid UTF-8 in response body".to_string()))
    }

    /// Get the response body as bytes
    pub async fn bytes(self) -> Result<Bytes, Error> {
        use http_body_util::BodyExt;

        let body_bytes = self
            .inner
            .into_body()
            .collect()
            .await
            .map_err(|_| Error::Platform("Failed to read response body".to_string()))?
            .to_bytes();

        Ok(body_bytes)
    }

    /// Deserialize the response body as JSON
    #[cfg(feature = "json")]
    pub async fn json<T: DeserializeOwned>(self) -> Result<T, Error> {
        let bytes = self.bytes().await?;
        serde_json::from_slice(&bytes).map_err(|_| Error::Platform("JSON deserialization failed".to_string()))
    }

    /// Get the content length
    pub fn content_length(&self) -> Option<u64> {
        self.inner.headers().get(http::header::CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
    }
}

/// Error type for HTTP operations
#[derive(Debug)]
pub enum Error {
    /// HTTP protocol error
    Http(http::Error),
    /// Network/IO error
    Network(String),
    /// Invalid URI
    InvalidUri(http::uri::InvalidUri),
    /// Platform-specific error
    Platform(String),
    /// Request timeout
    Timeout,
    /// Invalid header value
    InvalidHeader(http::header::InvalidHeaderValue),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Http(e) => write!(f, "HTTP error: {e}"),
            Error::Network(e) => write!(f, "Network error: {e}"),
            Error::InvalidUri(e) => write!(f, "Invalid URI: {e}"),
            Error::Platform(e) => write!(f, "Platform error: {e}"),
            Error::Timeout => write!(f, "Request timeout"),
            Error::InvalidHeader(e) => write!(f, "Invalid header: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<http::Error> for Error {
    fn from(err: http::Error) -> Self {
        Error::Http(err)
    }
}

impl From<http::uri::InvalidUri> for Error {
    fn from(err: http::uri::InvalidUri) -> Self {
        Error::InvalidUri(err)
    }
}

impl From<http::header::InvalidHeaderValue> for Error {
    fn from(err: http::header::InvalidHeaderValue) -> Self {
        Error::InvalidHeader(err)
    }
}

// Convenience functions (reqwest-compatible)

/// Make a GET request to the specified URL
pub async fn get<U: Into<Uri>>(url: U) -> Result<ResponseWrapper, Error> {
    Client::new().get(url).send().await
}

/// Make a POST request to the specified URL with a body
pub fn post<U: Into<Uri>>(url: U) -> RequestBuilder {
    Client::new().request(Method::POST, url.into())
}

// Keep the existing Tower Service implementation and ResponseBody...
// [Previous ResponseBody, ResponseFuture, and Tower Service implementations
// remain the same]

/// HTTP response body that implements http-body::Body
#[pin_project]
#[derive(Debug)]
pub struct ResponseBody {
    #[pin]
    inner: ResponseBodyInner,
}

#[pin_project(project = ResponseBodyInnerProj)]
#[derive(Debug)]
enum ResponseBodyInner {
    Ready { data: Option<Bytes> },
    Error { error: Option<Error> },
}

impl Body for ResponseBody {
    type Data = Bytes;
    type Error = Error;

    fn poll_frame(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        match this.inner.project() {
            ResponseBodyInnerProj::Ready { data } => {
                if let Some(bytes) = data.take() {
                    Poll::Ready(Some(Ok(Frame::data(bytes))))
                } else {
                    Poll::Ready(None)
                }
            }
            ResponseBodyInnerProj::Error { error } => {
                if let Some(err) = error.take() {
                    Poll::Ready(Some(Err(err)))
                } else {
                    Poll::Ready(None)
                }
            }
        }
    }
}

impl ResponseBody {
    fn from_data(data: Bytes) -> Self {
        Self {
            inner: ResponseBodyInner::Ready { data: Some(data) },
        }
    }

    fn from_error(error: Error) -> Self {
        Self {
            inner: ResponseBodyInner::Error { error: Some(error) },
        }
    }
}

/// Tower Service implementation for the HTTP client
impl Service<Request<Full<Bytes>>> for Client {
    type Response = Response<ResponseBody>;
    type Error = Error;
    type Future = ResponseFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut request: Request<Full<Bytes>>) -> Self::Future {
        // Add default headers
        for (name, value) in &self.config.default_headers {
            if !request.headers().contains_key(name) {
                request.headers_mut().insert(name.clone(), value.clone());
            }
        }

        // Add user agent if not present
        if let Some(ref user_agent) = self.config.user_agent {
            if !request.headers().contains_key("user-agent") {
                if let Ok(value) = HeaderValue::from_str(user_agent) {
                    request.headers_mut().insert("user-agent", value);
                }
            }
        }

        ResponseFuture::new(request, &self.inner, &self.config)
    }
}

/// Future representing a pending HTTP response
#[pin_project]
pub struct ResponseFuture {
    state: Arc<Mutex<ResponseState>>,
}

enum ResponseState {
    Pending { waker: Option<Waker> },
    Ready(Result<Response<ResponseBody>, Error>),
    Taken,
}

impl Future for ResponseFuture {
    type Output = Result<Response<ResponseBody>, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.lock().unwrap();

        match std::mem::replace(&mut *state, ResponseState::Taken) {
            ResponseState::Ready(result) => Poll::Ready(result),
            ResponseState::Pending { .. } => {
                *state = ResponseState::Pending {
                    waker: Some(cx.waker().clone()),
                };
                Poll::Pending
            }
            ResponseState::Taken => panic!("Future polled after completion"),
        }
    }
}

impl ResponseFuture {
    fn new(request: Request<Full<Bytes>>, client: &platform::PlatformClient, config: &ClientConfig) -> Self {
        let state = Arc::new(Mutex::new(ResponseState::Pending { waker: None }));
        let state_clone = state.clone();

        // Execute the platform-specific request
        match client.execute_request(request, config, move |result| {
            let mut state = state_clone.lock().unwrap();
            let waker = match &*state {
                ResponseState::Pending { waker } => waker.clone(),
                _ => None,
            };

            *state = ResponseState::Ready(result);

            if let Some(waker) = waker {
                waker.wake();
            }
        }) {
            Ok(()) => {}
            Err(e) => {
                *state.lock().unwrap() = ResponseState::Ready(Err(e));
            }
        }

        Self { state }
    }
}

// Platform-specific implementations
#[cfg(target_os = "macos")]
use macos as platform;

// Platform-specific module implementations
#[cfg(target_os = "macos")]
mod macos {
    use {
        super::{Bytes, ClientConfig, Error, Full, Request, Response, ResponseBody, StatusCode, http_body_util},
        block2::RcBlock,
        http_body_util::BodyExt,
        objc2::rc::Retained,
        objc2_foundation::{NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURL, NSURLResponse, NSURLSession},
    };

    #[derive(Clone, Debug)]
    pub struct PlatformClient {
        session: Retained<NSURLSession>,
    }

    impl PlatformClient {
        pub fn new(_config: &ClientConfig) -> Self {
            let session = unsafe { NSURLSession::sharedSession() };
            Self { session }
        }

        pub fn execute_request<F>(&self, request: Request<Full<Bytes>>, _config: &ClientConfig, callback: F) -> Result<(), Error>
        where
            F: Fn(Result<Response<ResponseBody>, Error>) + Send + 'static,
        {
            // Convert URI to NSURL
            let uri_str = request.uri().to_string();
            let ns_url = unsafe { NSURL::URLWithString(&NSString::from_str(&uri_str)).ok_or_else(|| Error::Platform("Invalid URL".to_string())) }?;

            // Create mutable request
            let ns_request = unsafe { NSMutableURLRequest::requestWithURL(&ns_url) };

            unsafe {
                // Set method
                let method_str = request.method().as_str();
                ns_request.setHTTPMethod(&NSString::from_str(method_str));

                // Set headers
                for (name, value) in request.headers() {
                    let name_str = name.as_str();
                    let value_str = std::str::from_utf8(value.as_bytes()).map_err(|_| Error::Platform("Invalid header value".to_string()))?;

                    ns_request.setValue_forHTTPHeaderField(Some(&NSString::from_str(value_str)), &NSString::from_str(name_str));
                }

                // Set body
                let body_future = request.into_body().collect();
                let body_data = futures::executor::block_on(body_future)
                    .map_err(|_| Error::Platform("Body collection failed".to_string()))?
                    .to_bytes();

                if !body_data.is_empty() {
                    let ns_data = NSData::with_bytes(&body_data);
                    ns_request.setHTTPBody(Some(&ns_data));
                }
            }

            // Create completion handler
            let completion_handler = RcBlock::new(move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
                // Safely convert raw pointers to Option references
                let data_ref = if data.is_null() { None } else { Some(unsafe { &*data }) };
                let response_ref = if response.is_null() { None } else { Some(unsafe { &*response }) };
                let error_ref = if error.is_null() { None } else { Some(unsafe { &*error }) };

                let result = if let Some(error) = error_ref {
                    Err(Error::Network(error.localizedDescription().to_string()))
                } else if let Some(ns_response) = response_ref {
                    convert_response(ns_response, data_ref)
                } else {
                    Err(Error::Network("No response received".to_string()))
                };

                callback(result);
            });

            // Start the task
            let task = unsafe { self.session.dataTaskWithRequest_completionHandler(&ns_request, &completion_handler) };
            unsafe { task.resume() };

            Ok(())
        }
    }

    fn convert_response(ns_response: &NSURLResponse, data: Option<&NSData>) -> Result<Response<ResponseBody>, Error> {
        let mut builder = Response::builder();

        // Extract status code and headers if it's an HTTP response
        if let Some(http_response) = ns_response.downcast_ref::<NSHTTPURLResponse>() {
            let status_code = unsafe { http_response.statusCode() } as u16;
            builder = builder.status(StatusCode::from_u16(status_code).map_err(|_| Error::Platform("Invalid status code".to_string()))?);

            // TODO: Extract headers from NSHTTPURLResponse.allHeaderFields
        }

        // Convert body data
        let body_bytes = if let Some(data) = data {
            let len = data.len();
            if len > 0 {
                let slice = unsafe { data.as_bytes_unchecked() };
                Bytes::copy_from_slice(slice)
            } else {
                Bytes::new()
            }
        } else {
            Bytes::new()
        };

        let response = builder.body(ResponseBody::from_data(body_bytes)).map_err(Error::Http)?;

        Ok(response)
    }
}

#[cfg(target_os = "windows")]
use windows as platform;
#[cfg(target_os = "windows")]
mod windows {
    use super::*;

    #[derive(Clone)]
    pub struct PlatformClient {
        // TODO: Windows HTTP API implementation
    }

    impl PlatformClient {
        pub fn new(_config: &ClientConfig) -> Self {
            Self {}
        }

        pub fn execute_request<F>(&self, _request: Request<Full<Bytes>>, _config: &ClientConfig, callback: F) -> Result<(), Error>
        where
            F: Fn(Result<Response<ResponseBody>, Error>) + Send + 'static,
        {
            // TODO: Implement using WinHTTP API
            callback(Err(Error::Platform("Windows implementation not yet available".to_string())));
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    #[derive(Clone)]
    pub struct PlatformClient {
        // TODO: Linux implementation using curl or io_uring
    }

    impl PlatformClient {
        pub fn new(_config: &ClientConfig) -> Self {
            Self {}
        }

        pub fn execute_request<F>(&self, _request: Request<Full<Bytes>>, _config: &ClientConfig, callback: F) -> Result<(), Error>
        where
            F: Fn(Result<Response<ResponseBody>, Error>) + Send + 'static,
        {
            // TODO: Implement using libcurl or io_uring
            callback(Err(Error::Platform("Linux implementation not yet available".to_string())));
            Ok(())
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
use fallback as platform;
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod fallback {
    use super::*;

    #[derive(Clone)]
    pub struct PlatformClient {}

    impl PlatformClient {
        pub fn new(_config: &ClientConfig) -> Self {
            Self {}
        }

        pub fn execute_request<F>(&self, _request: Request<Full<Bytes>>, _config: &ClientConfig, callback: F) -> Result<(), Error>
        where
            F: Fn(Result<Response<ResponseBody>, Error>) + Send + 'static,
        {
            callback(Err(Error::Platform("Platform not supported".to_string())));
            Ok(())
        }
    }
}
