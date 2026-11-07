//! A find-my-way style router: per-method route lists, static segments win over `:params`,
//! no trailing-slash folding; every route carries its rate-limit class and body parsers.

use crate::ctx::C;
use crate::error::ApiResult;
use crate::http::{BodyScope, Req};
use crate::security::Limit;
use axum::body::Body;
use axum::http::Response;
use futures_util::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;

/// A route handler.
pub type Handler =
    Arc<dyn Fn(C, Req) -> BoxFuture<'static, ApiResult<Response<Body>>> + Send + Sync>;

/// wrap an async fn as a [`Handler`]
pub fn h<F, Fut>(f: F) -> Handler
where
    F: Fn(C, Req) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ApiResult<Response<Body>>> + Send + 'static,
{
    Arc::new(move |c, r| Box::pin(f(c, r)))
}

#[derive(Debug, Clone, PartialEq)]
enum Seg {
    Static(String),
    Param(String),
}

/// One route.
#[derive(Clone)]
pub struct Route {
    pub method: &'static str,
    pub pattern: String,
    segs: Vec<Seg>,
    pub limit: Limit,
    pub scope: BodyScope,
    pub body_limit: Option<u64>,
    pub handler: Handler,
    /// static web file (no body parsing / device hook)
    pub is_static: bool,
}

/// The table.
#[derive(Default, Clone)]
pub struct Router {
    routes: HashMap<&'static str, Vec<Route>>,
}

fn split(p: &str) -> Vec<&str> {
    p.strip_prefix('/').unwrap_or(p).split('/').collect()
}

impl Router {
    pub fn add(
        &mut self,
        method: &'static str,
        pattern: &str,
        limit: Limit,
        scope: BodyScope,
        handler: Handler,
    ) -> &mut Route {
        let segs = split(pattern)
            .into_iter()
            .map(|s| match s.strip_prefix(':') {
                Some(n) => Seg::Param(n.to_string()),
                None => Seg::Static(s.to_string()),
            })
            .collect();
        let v = self.routes.entry(method).or_default();
        v.push(Route {
            method,
            pattern: pattern.to_string(),
            segs,
            limit,
            scope,
            body_limit: None,
            handler,
            is_static: false,
        });
        v.last_mut().expect("pushed")
    }

    /// find the route for a request path (percent-decoded params)
    pub fn find(&self, method: &str, path: &str) -> Option<(&Route, HashMap<String, String>)> {
        let m = if method == "HEAD" { "GET" } else { method };
        let list = self.routes.get(m)?;
        let parts = split(path);
        let mut best: Option<(&Route, Vec<u8>)> = None;
        for r in list {
            if r.segs.len() != parts.len() {
                continue;
            }
            let mut score = Vec::with_capacity(parts.len());
            let mut ok = true;
            for (s, p) in r.segs.iter().zip(&parts) {
                match s {
                    Seg::Static(x) => {
                        if x != p {
                            ok = false;
                            break;
                        }
                        score.push(0);
                    }
                    Seg::Param(_) => {
                        if p.is_empty() || p.len() > 500 {
                            ok = false;
                            break;
                        }
                        score.push(1);
                    }
                }
            }
            if ok && best.as_ref().is_none_or(|(_, b)| score < *b) {
                best = Some((r, score));
            }
        }
        let (r, _) = best?;
        let mut params = HashMap::new();
        for (s, p) in r.segs.iter().zip(&parts) {
            if let Seg::Param(n) = s {
                params.insert(
                    n.clone(),
                    percent_encoding::percent_decode_str(p)
                        .decode_utf8_lossy()
                        .into_owned(),
                );
            }
        }
        Some((r, params))
    }

    pub fn len(&self) -> usize {
        self.routes.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
