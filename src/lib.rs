//! Hubcaps provides a set of building blocks for interacting with the Github API
//!
//! # Examples
//!
//!  Typical use will require instantiation of a Github client. Which requires
//! a user agent string and set of `hubcaps::Credentials`.
//!
//! ```no_run
//! extern crate hubcaps;
//! extern crate hyper;
//!
//! use hubcaps::{Credentials, Github};
//!
//! fn main() {
//!   let github = Github::new(
//!     String::from("user-agent-name"),
//!     Credentials::Token(
//!       String::from("personal-access-token")
//!     ),
//!   );
//! }
//! ```
//!
//! Github enterprise users will want to create a client with the
//! [Github#host](struct.Github.html#method.host) method
//!
//! Access to various services are provided via methods on instances of the `Github` type.
//!
//! The convention for executing operations typically looks like
//! `github.repo(.., ..).service().operation(OperationOptions)` where operation may be `create`,
//! `delete`, etc.
//!
//! Services and their types are packaged under their own module namespace.
//! A service interface will provide access to operations and operations may access options types
//! that define the various parameter options available for the operation. Most operation option
//! types expose `builder()` methods for a builder oriented style of constructing options.
//!
//! ## Entity listings
//!
//! Many of Github's APIs return a collection of entities with a common interface for supporting pagination
//! Hubcaps supports two types of interfaces for working with listings. `list(...)` interfaces return the first
//! ( often enough ) list of entities. Alternatively for listings that require > 30 items you may wish to
//! use the `iter(..)` variant which returns a `futures::Stream` over all entities in a paginated set.
//!
//! # Errors
//!
//! Operations typically result in a `hubcaps::Future` with an error type pinned to
//! [hubcaps::Error](errors/struct.Error.html).
//!
//! ## Rate Limiting
//!
//! A special note should be taken when accounting for Github's
//! [API Rate Limiting](https://developer.github.com/v3/rate_limit/)
//! A special case
//! [hubcaps::ErrorKind::RateLimit](errors/enum.ErrorKind.html#variant.RateLimit)
//! will be returned from api operations when the rate limit
//! associated with credentials has been exhausted. This type will include a reset
//! Duration to wait before making future requests.
//!
//! This crate uses the `log` crate's debug log interface to log x-rate-limit
//! headers received from Github.
//! If you are attempting to test your access patterns against
//! Github's rate limits, enable debug looking and look for "x-rate-limit"
//! log patterns sourced from this crate
//!
#![allow(missing_docs)] // todo: make this a deny eventually

#[macro_use]
extern crate error_chain;
extern crate futures;
extern crate http;
extern crate hyper;
#[cfg(feature = "tls")]
extern crate hyper_tls;
extern crate hyperx;
#[macro_use]
extern crate log;
extern crate mime;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate url;

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::{future, stream, Future as StdFuture, IntoFuture, Stream as StdStream};
use hyper::client::connect::Connect;
use hyper::client::HttpConnector;
use hyper::header::{ACCEPT, AUTHORIZATION, LINK, LOCATION, USER_AGENT};
use hyper::{Body, Client, Method, Request, StatusCode, Uri};
#[cfg(feature = "tls")]
use hyper_tls::HttpsConnector;
use hyperx::header::{qitem, Link, RelationType};
use mime::Mime;
use serde::de::DeserializeOwned;
use url::Url;

#[macro_use]
mod macros; // expose json! macro to child modules
pub mod activity;
pub mod branches;
pub mod comments;
pub mod deployments;
pub mod errors;
pub mod gists;
pub mod git;
pub mod hooks;
pub mod issues;
pub mod keys;
pub mod labels;
pub mod notifications;
pub mod organizations;
pub mod pull_commits;
pub mod pulls;
pub mod rate_limit;
pub mod releases;
pub mod repositories;
pub mod review_comments;
pub mod search;
pub mod stars;
pub mod statuses;
pub mod teams;
pub mod traffic;
pub mod users;

pub use errors::{Error, ErrorKind, Result};

use activity::Activity;
use gists::{Gists, UserGists};
use organizations::{Organization, Organizations, UserOrganizations};
use rate_limit::RateLimit;
use repositories::{OrganizationRepositories, Repositories, Repository, UserRepositories};
use search::Search;
use users::Users;

const DEFAULT_HOST: &str = "https://api.github.com";

/// A type alias for `Futures` that may return `hubcaps::Errors`
pub type Future<T> = Box<StdFuture<Item = T, Error = Error> + Send>;

/// A type alias for `Streams` that may result in `hubcaps::Errors`
pub type Stream<T> = Box<StdStream<Item = T, Error = Error> + Send>;

const X_GITHUB_REQUEST_ID: &str = "x-github-request-id";
const X_RATELIMIT_LIMIT: &str = "x-ratelimit-limit";
const X_RATELIMIT_REMAINING: &str = "x-ratelimit-remaining";
const X_RATELIMIT_RESET: &str = "x-ratelimit-reset";

/// Github defined Media types
/// See [this doc](https://developer.github.com/v3/media/) for more for more information
#[derive(Clone, Copy)]
pub enum MediaType {
    /// Return json (the default)
    Json,
    /// Return json in preview form
    Preview(&'static str),
}

impl Default for MediaType {
    fn default() -> MediaType {
        MediaType::Json
    }
}

impl From<MediaType> for Mime {
    fn from(media: MediaType) -> Mime {
        match media {
            MediaType::Json => "application/vnd.github.v3+json".parse().unwrap(),
            MediaType::Preview(codename) => {
                format!("application/vnd.github.{}-preview+json", codename)
                    .parse()
                    .unwrap_or_else(|_| panic!("could not parse media type for preview {}", codename))
            }
        }
    }
}

/// enum representation of Github list sorting options
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SortDirection {
    /// Sort in ascending order (the default)
    Asc,
    /// Sort in descending order
    Desc,
}

impl fmt::Display for SortDirection {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            SortDirection::Asc => "asc",
            SortDirection::Desc => "desc",
        }.fmt(f)
    }
}

impl Default for SortDirection {
    fn default() -> SortDirection {
        SortDirection::Asc
    }
}

/// Various forms of authentication credentials supported by Github
#[derive(Debug, PartialEq, Clone)]
pub enum Credentials {
    /// Oauth token string
    /// https://developer.github.com/v3/#oauth2-token-sent-in-a-header
    Token(String),
    /// Oauth client id and secret
    /// https://developer.github.com/v3/#oauth2-keysecret
    Client(String, String),
}

/// Entry point interface for interacting with Github API
#[derive(Clone, Debug)]
pub struct Github<C>
where
    C: Clone + Connect + 'static,
{
    host: String,
    agent: String,
    client: Client<C>,
    credentials: Option<Credentials>,
}

#[cfg(feature = "tls")]
impl Github<HttpsConnector<HttpConnector>> {
    pub fn new<A, C>(agent: A, credentials: C) -> Self
    where
        A: Into<String>,
        C: Into<Option<Credentials>>,
    {
        Self::host(DEFAULT_HOST, agent, credentials)
    }

    pub fn host<H, A, C>(host: H, agent: A, credentials: C) -> Self
    where
        H: Into<String>,
        A: Into<String>,
        C: Into<Option<Credentials>>,
    {
        let connector = HttpsConnector::new(4).unwrap();
        let http = Client::builder()
            .keep_alive(true)
            .build(connector);
        Self::custom(host, agent, credentials, http)
    }
}

impl<C> Github<C>
where
    C: Clone + Connect + 'static,
{
    pub fn custom<H, A, CR>(host: H, agent: A, credentials: CR, http: Client<C>) -> Self
    where
        H: Into<String>,
        A: Into<String>,
        CR: Into<Option<Credentials>>,
    {
        Self {
            host: host.into(),
            agent: agent.into(),
            client: http,
            credentials: credentials.into(),
        }
    }

    pub fn rate_limit(&self) -> RateLimit<C> {
        RateLimit::new(self.clone())
    }

    /// Return a reference to user activity
    pub fn activity(&self) -> Activity<C> {
        Activity::new(self.clone())
    }

    /// Return a reference to a Github repository
    pub fn repo<O, R>(&self, owner: O, repo: R) -> Repository<C>
    where
        O: Into<String>,
        R: Into<String>,
    {
        Repository::new(self.clone(), owner, repo)
    }

    /// Return a reference to the collection of repositories owned by and
    /// associated with an owner
    pub fn user_repos<S>(&self, owner: S) -> UserRepositories<C>
    where
        S: Into<String>,
    {
        UserRepositories::new(self.clone(), owner)
    }

    /// Return a reference to the collection of repositories owned by the user
    /// associated with the current authentication credentials
    pub fn repos(&self) -> Repositories<C> {
        Repositories::new(self.clone())
    }

    pub fn org<O>(&self, org: O) -> Organization<C>
    where
        O: Into<String>,
    {
        Organization::new(self.clone(), org)
    }

    /// Return a reference to the collection of organizations that the user
    /// associated with the current authentication credentials is in
    pub fn orgs(&self) -> Organizations<C> {
        Organizations::new(self.clone())
    }

    /// Return a reference to an interface that provides access
    /// to user information.
    pub fn users(&self) -> Users<C> {
        Users::new(self.clone())
    }

    /// Return a reference to the collection of organizations a user
    /// is publicly associated with
    pub fn user_orgs<U>(&self, user: U) -> UserOrganizations<C>
    where
        U: Into<String>,
    {
        UserOrganizations::new(self.clone(), user)
    }

    /// Return a reference to an interface that provides access to a user's gists
    pub fn user_gists<O>(&self, owner: O) -> UserGists<C>
    where
        O: Into<String>,
    {
        UserGists::new(self.clone(), owner)
    }

    /// Return a reference to an interface that provides access to the
    /// gists belonging to the owner of the token used to configure this client
    pub fn gists(&self) -> Gists<C> {
        Gists::new(self.clone())
    }

    /// Return a reference to an interface that provides access to search operations
    pub fn search(&self) -> Search<C> {
        Search::new(self.clone())
    }

    /// Return a reference to the collection of repositories owned by and
    /// associated with an organization
    pub fn org_repos<O>(&self, org: O) -> OrganizationRepositories<C>
    where
        O: Into<String>,
    {
        OrganizationRepositories::new(self.clone(), org)
    }

    fn request<Out>(
        &self,
        method: Method,
        uri: &str,
        body: Option<Vec<u8>>,
        media_type: MediaType,
    ) -> Future<(Option<Link>, Out)>
    where
        Out: DeserializeOwned + 'static + Send,
    {
        let url = if let Some(Credentials::Client(ref id, ref secret)) = self.credentials {
            let mut parsed = Url::parse(&uri).unwrap();
            parsed
                .query_pairs_mut()
                .append_pair("client_id", id)
                .append_pair("client_secret", secret);
            parsed.to_string().parse::<Uri>().into_future()
        } else {
            uri.parse().into_future()
        };
        let instance = self.clone();
        let body2 = body.clone();
        let method2 = method.clone();
        let response = url.map_err(Error::from).and_then(move |url| {
            let mut req = Request::builder();
            req.method(method2).uri(url);

            req.header(USER_AGENT, &*instance.agent);
            req.header(ACCEPT, &*format!("{}", qitem::<Mime>(From::from(media_type))));

            if let Some(Credentials::Token(ref token)) = instance.credentials {
                req.header(AUTHORIZATION, &*format!("token {}", token));
            }

            let req = match body2 {
                Some(body) => req.body(Body::from(body)),
                None => req.body(Body::empty()),
            };

            req.map_err(Error::from)
                .into_future()
                .and_then(move |req| instance.client.request(req).map_err(Error::from))
        });
        let instance2 = self.clone();
        Box::new(response.and_then(move |response| {
            if let Some(value) = response.headers().get(X_GITHUB_REQUEST_ID) {
                debug!("x-github-request-id: {:?}", value)
            }
            if let Some(value) = response.headers().get(X_RATELIMIT_LIMIT) {
                debug!("x-rate-limit-limit: {:?}", value)
            }
            let remaining = response
                .headers()
                .get(X_RATELIMIT_REMAINING)
                .and_then(|val| val.to_str().ok())
                .and_then(|val| val.parse::<u32>().ok());
            let reset = response
                .headers()
                .get(X_RATELIMIT_RESET)
                .and_then(|val| val.to_str().ok())
                .and_then(|val| val.parse::<u32>().ok());
            if let Some(value) = remaining {
                debug!("x-rate-limit-remaining: {}", value)
            }
            if let Some(value) = reset {
                debug!("x-rate-limit-reset: {}", value)
            }
            let status = response.status();
            // handle redirect common with renamed repos
            if StatusCode::MOVED_PERMANENTLY == status || StatusCode::TEMPORARY_REDIRECT == status {
                let location = response.headers().get(LOCATION)
                    .and_then(|l| l.to_str().ok());

                if let Some(location) = location {
                    debug!("redirect location {:?}", location);
                    return instance2.request(method, &location.to_string(), body, media_type);
                }
            }
            let link = response
                .headers()
                .get(LINK)
                .and_then(|l| l.to_str().ok())
                .and_then(|l| l.parse().ok());

            Box::new(response.into_body().concat2().map_err(Error::from).and_then(
                move |response_body| {
                    if status.is_success() {
                        debug!(
                            "response payload {}",
                            String::from_utf8_lossy(&response_body)
                        );
                        serde_json::from_slice::<Out>(&response_body)
                            .map(|out| (link, out))
                            .map_err(|error| ErrorKind::Codec(error).into())
                    } else {
                        let error = match (remaining, reset) {
                            (Some(remaining), Some(reset)) if remaining == 0 => {
                                let now = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs();
                                ErrorKind::RateLimit {
                                    reset: Duration::from_secs(u64::from(reset) - now),
                                }
                            }
                            _ => ErrorKind::Fault {
                                code: status,
                                error: serde_json::from_slice(&response_body)?,
                            },
                        };
                        Err(error.into())
                    }
                },
            ))
        }))
    }

    fn request_entity<D>(
        &self,
        method: Method,
        uri: &str,
        body: Option<Vec<u8>>,
        media_type: MediaType,
    ) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
    {
        Box::new(
            self.request(method, uri, body, media_type)
                .map(|(_, entity)| entity),
        )
    }

    fn get<D>(&self, uri: &str) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
    {
        self.get_media(uri, MediaType::Json)
    }

    fn get_media<D>(&self, uri: &str, media: MediaType) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
    {
        self.request_entity(Method::GET, &(self.host.clone() + uri), None, media)
    }

    fn get_pages<D>(&self, uri: &str) -> Future<(Option<Link>, D)>
    where
        D: DeserializeOwned + 'static + Send,
    {
        self.request(Method::GET, &(self.host.clone() + uri), None, MediaType::Json)
    }

    fn delete(&self, uri: &str) -> Future<()> {
        Box::new(self.request_entity::<()>(
            Method::DELETE,
            &(self.host.clone() + uri),
            None,
            MediaType::Json,
        ).or_else(|err| match err {
            Error(ErrorKind::Codec(_), _) => Ok(()),
            otherwise => Err(otherwise),
        }))
    }

    fn post<D>(&self, uri: &str, message: Vec<u8>) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
    {
        self.request_entity(
            Method::POST,
            &(self.host.clone() + uri),
            Some(message),
            MediaType::Json,
        )
    }

    fn patch_no_response(&self, uri: &str, message: Vec<u8>) -> Future<()> {
        Box::new(self.patch(uri, message).or_else(|err| match err {
            Error(ErrorKind::Codec(_), _) => Ok(()),
            err => Err(err),
        }))
    }

    fn patch_media<D>(&self, uri: &str, message: Vec<u8>, media: MediaType) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
    {
        self.request_entity(Method::PATCH, &(self.host.clone() + uri), Some(message), media)
    }

    fn patch<D>(&self, uri: &str, message: Vec<u8>) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
    {
        self.patch_media(uri, message, MediaType::Json)
    }

    fn put_no_response(&self, uri: &str, message: Vec<u8>) -> Future<()> {
        Box::new(self.put(uri, message).or_else(|err| match err {
            Error(ErrorKind::Codec(_), _) => Ok(()),
            err => Err(err),
        }))
    }

    fn put<D>(&self, uri: &str, message: Vec<u8>) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
    {
        self.request_entity(
            Method::PUT,
            &(self.host.clone() + uri),
            Some(message),
            MediaType::Json,
        )
    }
}

fn next_link(l: &Link) -> Option<String> {
    l.values()
        .into_iter()
        .find(|v| v.rel().unwrap_or(&[]).get(0) == Some(&RelationType::Next))
        .map(|v| v.link().to_owned())
}

/// "unfold" paginated results of a list of github entities
fn unfold<C, D, I>(
    github: Github<C>,
    first: Future<(Option<Link>, D)>,
    into_items: fn(D) -> Vec<I>,
) -> Stream<I>
where
    D: DeserializeOwned + 'static + Send,
    I: 'static + Send,
    C: Clone + Connect + 'static,
{
    Box::new(
        first
            .map(move |(link, payload)| {
                let mut items = into_items(payload);
                items.reverse();
                stream::unfold::<_, _, Future<(I, (Option<Link>, Vec<I>))>, _>(
                    (link, items),
                    move |(link, mut items)| match items.pop() {
                        Some(item) => Some(Box::new(future::ok((item, (link, items))))),
                        _ => link.and_then(|l| next_link(&l)).map(|url| {
                            let url = Url::parse(&url).unwrap();
                            let uri = [url.path(), url.query().unwrap_or_default()].join("?");
                            Box::new(github.get_pages(uri.as_ref()).map(move |(link, payload)| {
                                let mut items = into_items(payload);
                                items.reverse();
                                (items.remove(0), (link, items))
                            })) as Future<(I, (Option<Link>, Vec<I>))>
                        }),
                    },
                )
            })
            .into_stream()
            .flatten(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sort_direction() {
        let default: SortDirection = Default::default();
        assert_eq!(default, SortDirection::Asc)
    }
}
