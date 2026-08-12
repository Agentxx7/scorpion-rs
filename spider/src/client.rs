/// The asynchronous Client to make requests with.
#[cfg(not(feature = "wreq"))]
pub type Client = reqwest::Client;
#[cfg(not(feature = "wreq"))]
/// The asynchronous Client Builder.
pub type ClientBuilder = reqwest::ClientBuilder;
#[cfg(not(feature = "wreq"))]
pub use reqwest as request_client;
#[cfg(all(feature = "cookies", not(feature = "wreq")))]
pub use reqwest::cookie;
#[cfg(not(feature = "wreq"))]
pub use reqwest::{dns, header, redirect, Error, Proxy, Response, StatusCode};

/// The asynchronous Client to make requests with wreq.
#[cfg(feature = "wreq")]
pub type Client = wreq::Client;
#[cfg(feature = "wreq")]
/// The asynchronous Client Builder.
pub type ClientBuilder = wreq::ClientBuilder;
#[cfg(feature = "wreq")]
pub use wreq as request_client;
#[cfg(all(feature = "cookies", feature = "wreq"))]
pub use wreq::cookie;
#[cfg(feature = "wreq")]
pub use wreq::{dns, header, redirect, Error, Proxy, Response, StatusCode};
#[cfg(feature = "wreq")]
pub use wreq_util;
