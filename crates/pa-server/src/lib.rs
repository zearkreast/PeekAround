//! PeekAround server (mirrors `packages/server`): the same HTTP / WebSocket API, the same
//! SQLite database and data directory layout, on axum/hyper + tokio.
//!
//! ```no_run
//! # async fn demo() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let mut o = pa_server::ServerOptions::new("/tmp/peek-data");
//! o.port = 0;
//! let srv = pa_server::Server::start(o).await?;
//! println!("{}", srv.url);
//! srv.close().await;
//! # Ok(()) }
//! ```
//!
//! Module map (TS → Rust): `app.ts` → [`Server`], `config.ts` → [`config`], `context.ts` +
//! `runtime-settings.ts` → [`ctx`], `db.ts` + `migrations/*` → [`db`], `security.ts` → [`security`],
//! `paths.ts` → [`paths`], `library.ts` → [`library`], `events.ts` → [`events`], `media/scanner.ts`
//! → [`scanner`], `media/watcher.ts` → [`watcher`], `media/thumbs.ts` + `background.ts` → [`thumbs`]
//! (+ pa-media), `explore.ts` + `duplicates.ts` → [`explore`], `insights.ts` + the query half of
//! `routes/browse.ts` → [`scope`], `geo.ts` → [`geo`], `tiles.ts` → [`tiles`], `zip.ts` → [`zip`],
//! `routes/*` → [`routes`], `jobs/*` → pa-jobs (adapters in [`engines`]), `devices/{hub,access}.ts` (state) →
//! [`devices`], `remote/folders.ts` (state) → [`remote`].

// tuples of maps / callbacks are spelled out where they are used (watcher, scanner write queue)
#![allow(clippy::type_complexity)]

pub mod config;
mod country_names;
pub mod ctx;
pub mod db;
pub mod devices;
pub mod engines;
pub mod error;
pub mod events;
pub mod exifw;
pub mod explore;
pub mod fonts;
pub mod geo;
pub mod http;
pub mod library;
pub mod mdns;
pub mod mime;
pub mod paths;
pub mod remote;
pub mod router;
pub mod routes;
pub mod scanner;
pub mod scope;
pub mod security;
pub mod thumbs;
pub mod tiles;
pub mod watcher;
pub mod zip;

pub use config::{DesktopHooks, RootSpec, SecurityOptions, ServerOptions};
pub use ctx::{Ctx, C};

use axum::body::Body;
use axum::http::{header, HeaderValue, Request, Response, StatusCode};
use http::ConnInfo;
use pa_core::{LanInfo, MediaReason, RunMode, ServerEvent};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A boxed error of [`Server::start`].
pub type StartError = Box<dyn std::error::Error + Send + Sync>;

struct App {
    ctx: C,
    router: router::Router,
    web_dir: Option<PathBuf>,
    /// '/<file>' (and '/<dir>/' for index.html) → file
    web_files: HashMap<String, PathBuf>,
}

struct Listener {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

/// A running server (`PeekServer`).
pub struct Server {
    /// `http://<host>:<port>` of the main listener
    pub url: String,
    /// the port it listens on
    pub port: u16,
    /// shared context (tests / desktop app)
    pub ctx: C,
    app: Arc<App>,
    main: Mutex<Option<Listener>>,
    lan: tokio::sync::Mutex<Option<Listener>>,
    mdns: Mutex<Option<mdns::Mdns>>,
    cleanup: Mutex<Option<tokio::task::JoinHandle<()>>>,
    _subs: Vec<events::Subscription>,
}

fn json_error(status: u16, msg: &str) -> Response<Body> {
    error::HttpError::new(status, msg).into_response_inner()
}

impl error::HttpError {
    fn into_response_inner(self) -> Response<Body> {
        axum::response::IntoResponse::into_response(self)
    }
}

fn collect_web_files(dir: &std::path::Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    fn walk(d: &std::path::Path, rel: &str, out: &mut HashMap<String, PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let p = e.path();
            let r = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if p.is_dir() {
                walk(&p, &r, out);
            } else {
                out.insert(format!("/{r}"), p.clone());
                if name == "index.html" {
                    let dirk = if rel.is_empty() {
                        "/".to_string()
                    } else {
                        format!("/{rel}/")
                    };
                    out.entry(dirk).or_insert(p);
                }
            }
        }
    }
    walk(dir, "", &mut out);
    out
}

impl Server {
    /// `createServer(opts)`: open the database, start the services, listen.
    pub async fn start(opts: ServerOptions) -> Result<Server, StartError> {
        let cfg = config::ResolvedConfig::resolve(&opts);
        for d in [
            &cfg.data_dir,
            &cfg.cache_dir,
            &cfg.thumb_dir,
            &cfg.temp_dir,
            &cfg.font_dir,
        ] {
            std::fs::create_dir_all(d)?;
        }
        let geo = Arc::new(geo::GeoService::new(cfg.web_dir.as_deref()));
        let db = Arc::new(db::Db::open(&cfg.db_path, geo.clone())?);
        let remote_mb = db
            .get_setting("settings")
            .and_then(|v| v.get("remoteCacheMb").and_then(|x| x.as_f64()))
            .unwrap_or(pa_core::Settings::default().remote_cache_mb as f64);
        let events = events::EventHub::new();
        let ffmpeg = Arc::new(pa_media::Ffmpeg::new(
            cfg.ffmpeg_path.clone(),
            Some(PathBuf::from(&cfg.ffprobe_path)),
        ));
        ffmpeg.check().await;
        let thumbs = thumbs::ThumbService::new(cfg.thumb_dir.clone(), ffmpeg.clone());
        let temp_dir = cfg.temp_dir.clone();
        let version = cfg.version.clone();
        let tile_dir = cfg.cache_dir.join("tiles");
        let remote_dir = cfg.cache_dir.join("remote");
        let ctx: C = Arc::new_cyclic(|weak: &Weak<Ctx>| {
            // the engines and their adapters (engines.rs)
            let store = Arc::new(engines::Store {
                ctx: weak.clone(),
                db: db.clone(),
            });
            let sink: Arc<dyn pa_jobs::EventSink> = Arc::new(engines::ServerEvents {
                events: events.clone(),
                ctx: weak.clone(),
            });
            let jobs = pa_jobs::JobService::new(sink.clone(), &temp_dir);
            let link = pa_link::DeviceHub::new(pa_link::HubDeps {
                store: store.clone(),
                rows: store.clone(),
                thumbs: store.clone(),
                events: sink.clone(),
                server_name: config::hostname(),
            });
            let account =
                pa_remote::RemoteAccount::new(store.clone(), pa_mega::ApiOptions::default());
            let svc = pa_remote::RemoteService::new(pa_remote::RemoteDeps {
                store: store.clone(),
                rows: store.clone(),
                thumbs: store.clone(),
                events: sink.clone(),
                import: store.clone(),
                jobs: jobs.clone(),
                ffmpeg: ffmpeg.clone(),
                mega: Arc::new(account.client.clone()),
                cache_dir: remote_dir.clone(),
                cache_bytes: (remote_mb.max(0.0) as u64) * 1024 * 1024,
            });
            let w = weak.clone();
            let cap = Arc::new(move || {
                w.upgrade()
                    .map(|c| c.settings().tile_cache_mb)
                    .unwrap_or(512)
            });
            Ctx {
                security: opts
                    .security
                    .clone()
                    .unwrap_or_else(SecurityOptions::from_env),
                tiles: tiles::TileService::new(
                    tile_dir,
                    format!("PeekAround/{version} (+https://github.com/zearkreast/PeakAround)"),
                    cap,
                ),
                cfg,
                db,
                events: events.clone(),
                ffmpeg: ffmpeg.clone(),
                thumbs,
                geo,
                scanner: scanner::Scanner::new(),
                watcher: watcher::Watcher::default(),
                explore: explore::Explore::default(),
                devices: devices::DeviceHub::new(link),
                remote: remote::RemoteService {
                    cache_dir: remote_dir.clone(),
                    svc,
                    account,
                },
                jobs,
                exporter: std::sync::OnceLock::new(),
                rate: security::RateLimiter::default(),
                lan: Mutex::new((false, 8080)),
                shutdown: CancellationToken::new(),
                tasks: tokio_util::task::TaskTracker::new(),
                skeletons: Mutex::new(Default::default()),
                meta_memo: Mutex::new(None),
                rekick: std::sync::atomic::AtomicBool::new(false),
                local_tz: db::system_time_zone(),
            }
        });
        let _ = ctx
            .exporter
            .set(engines::exporter(&ctx, Arc::downgrade(&ctx)));
        ctx.remote.account.bind(&ctx.remote.svc);
        *ctx.thumbs.remote.lock() = Some(Arc::new(engines::DeviceImages(Arc::downgrade(&ctx))));
        *ctx.thumbs.cloud.lock() = Some(Arc::new(engines::CloudImages(Arc::downgrade(&ctx))));
        // home time zone: persisted on first run
        let stored_tz = ctx.db.get_setting("settings").and_then(|s| {
            s.get("homeTimeZone")
                .and_then(|v| v.as_str().map(String::from))
        });
        if !stored_tz
            .as_deref()
            .is_some_and(pa_core::is_valid_time_zone)
        {
            let mut s = ctx.settings_json();
            s.insert(
                "homeTimeZone".into(),
                serde_json::Value::String(db::system_time_zone()),
            );
            ctx.db
                .set_setting("settings", &serde_json::Value::Object(s))?;
        }
        let s = ctx.settings();
        ctx.scanner.set_show_hidden(s.show_hidden);
        ctx.scanner.init_home_time_zone(&s.home_time_zone);
        ctx.apply_runtime_settings(&s);

        // subscriptions: scanner rules, explore invalidation
        let mut subs = Vec::new();
        let rules = ctx.scanner.rules_handle();
        subs.push(ctx.events.on(move |e| {
            if let ServerEvent::Folders { status: None } = e {
                rules.lock().clear();
            }
        }));
        let w = Arc::downgrade(&ctx);
        subs.push(ctx.events.on(move |e| {
            let Some(c) = w.upgrade() else { return };
            match e {
                ServerEvent::Media { reason, .. } if *reason != Some(MediaReason::Thumb) => {
                    c.explore.invalidate(
                        &c,
                        matches!(reason, Some(MediaReason::Add) | Some(MediaReason::Remove)),
                    )
                }
                ServerEvent::Folders { status: None } => c.explore.invalidate(&c, false),
                _ => {}
            }
        }));

        let web_dir = ctx
            .cfg
            .web_dir
            .clone()
            .filter(|d| d.join("index.html").is_file());
        let web_files = web_dir
            .as_deref()
            .map(collect_web_files)
            .unwrap_or_default();
        let app = Arc::new(App {
            ctx: ctx.clone(),
            router: routes::router(),
            web_dir,
            web_files,
        });

        library::ensure_roots(&ctx)?;
        let _ = library::cleanup_temp(&ctx, false).await;
        for t in library::list_temp(&ctx).unwrap_or_default() {
            if !std::path::Path::new(&t.path).exists() {
                if let Ok(row) = library::folder_row_str(&ctx, &t.id) {
                    let _ = library::delete_temp(&ctx, &row).await;
                }
            }
        }
        let c2 = ctx.clone();
        let cleanup = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(600)) => { let _ = library::cleanup_temp(&c2, false).await; }
                    _ = c2.shutdown.cancelled() => break,
                }
            }
        });

        let host: IpAddr = ctx.cfg.host.parse().unwrap_or(IpAddr::from([0, 0, 0, 0]));
        let tcp = tokio::net::TcpListener::bind(SocketAddr::new(host, ctx.cfg.port)).await?;
        let port = tcp.local_addr()?.port();
        let shown = if ctx.cfg.host == "0.0.0.0" || ctx.cfg.host == "::" {
            "127.0.0.1".to_string()
        } else {
            ctx.cfg.host.clone()
        };
        let shown_url = if shown.contains(':') {
            format!("[{shown}]")
        } else {
            shown.clone()
        };
        let self_url = format!("http://{shown_url}:{port}");
        ctx.devices.set_self_url(self_url.clone());
        ctx.remote
            .svc
            .set_loopback(&self_url, &ctx.devices.internal_token);
        let main = spawn_listener(app.clone(), tcp, false);

        watcher::sync(&ctx);
        let c3 = ctx.clone();
        ctx.tasks.spawn(async move { scanner::scan_all(&c3).await });
        // remote (MEGA) folders: re-list in the background
        ctx.remote.svc.start();

        let srv = Server {
            url: format!("http://{shown}:{port}"),
            port,
            ctx: ctx.clone(),
            app,
            main: Mutex::new(Some(main)),
            lan: tokio::sync::Mutex::new(None),
            mdns: Mutex::new(None),
            cleanup: Mutex::new(Some(cleanup)),
            _subs: subs,
        };
        if ctx.cfg.mdns && ctx.cfg.mode == RunMode::Server {
            srv.publish_mdns(port);
        }
        Ok(srv)
    }

    fn publish_mdns(&self, port: u16) {
        let hostname = config::hostname();
        let version = self.ctx.cfg.version.clone();
        *self.mdns.lock() = mdns::Mdns::publish(&hostname, port, &version);
    }

    /// Desktop: expose the gallery on the local network on an extra port (requests are flagged
    /// as LAN: `requireLocal` routes answer 403, device folders follow the visibility rules).
    pub async fn set_lan(&self, enabled: bool, port: Option<u16>) -> Result<LanInfo, StartError> {
        let lan_port = port.unwrap_or(8080);
        let mut lan = self.lan.lock().await;
        if let Some(l) = lan.take() {
            l.cancel.cancel();
            let _ = l.task.await;
            // "stop sharing" cuts the phone tunnels that came in through the LAN listener too
            self.ctx.devices.link.close_lan();
            if let Some(m) = self.mdns.lock().take() {
                m.stop();
            }
        }
        *self.ctx.lan.lock() = (false, lan_port);
        if enabled {
            let tcp =
                tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], lan_port))).await?;
            *lan = Some(spawn_listener(self.app.clone(), tcp, true));
            *self.ctx.lan.lock() = (true, lan_port);
            if self.ctx.cfg.mdns {
                self.publish_mdns(lan_port);
            }
        }
        let info = routes::core::server_info(&self.ctx);
        self.ctx
            .events
            .emit(ServerEvent::Hello { info: info.clone() });
        Ok(info.lan)
    }

    /// the LAN listener is running
    pub async fn lan_running(&self) -> bool {
        self.lan.lock().await.is_some()
    }

    /// Answer a request in-process (Fastify `inject`): client 127.0.0.1, main listener.
    pub async fn inject(&self, req: Request<Body>) -> Response<Body> {
        dispatch(self.app.clone(), req, ConnInfo::default()).await
    }

    /// [`Server::inject`] with explicit connection facts (client IP, LAN flag).
    pub async fn inject_from(&self, req: Request<Body>, conn: ConnInfo) -> Response<Body> {
        dispatch(self.app.clone(), req, conn).await
    }

    /// Graceful shutdown: background loops are awaited before the database closes.
    pub async fn close(&self) {
        let ctx = &self.ctx;
        if let Some(c) = self.cleanup.lock().take() {
            c.abort();
        }
        if let Some(m) = self.mdns.lock().take() {
            m.stop();
        }
        if let Some(l) = self.lan.lock().await.take() {
            l.cancel.cancel();
            let _ = l.task.await;
        }
        ctx.shutdown.cancel();
        ctx.devices.link.close().await;
        ctx.remote.svc.close().await;
        ctx.jobs.close().await;
        watcher::close(ctx);
        scanner::stop(ctx);
        ctx.explore.close();
        ctx.tasks.close();
        let _ = tokio::time::timeout(Duration::from_secs(30), ctx.tasks.wait()).await;
        let main = self.main.lock().take();
        if let Some(l) = main {
            l.cancel.cancel();
            let _ = l.task.await;
        }
        ctx.events.close();
        ctx.db.close();
    }
}

fn spawn_listener(app: Arc<App>, tcp: tokio::net::TcpListener, lan: bool) -> Listener {
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let task = tokio::spawn(async move {
        let mut conns = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                acc = tcp.accept() => {
                    let Ok((stream, addr)) = acc else { continue };
                    let _ = stream.set_nodelay(true);
                    let ip = match addr.ip() {
                        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
                        v4 => v4,
                    };
                    let conn = ConnInfo { ip, lan, closed: token.clone() };
                    let app = app.clone();
                    conns.spawn(async move {
                        let io = hyper_util::rt::TokioIo::new(stream);
                        let svc = hyper::service::service_fn(move |req: Request<hyper::body::Incoming>| {
                            let app = app.clone();
                            let conn = conn.clone();
                            async move {
                                // Node's HTTP parser rejects raw (unencoded) non-ASCII bytes in the request target
                                if req.uri().path_and_query().is_some_and(|pq| !pq.as_str().is_ascii()) {
                                    return Ok::<_, std::convert::Infallible>(client_error());
                                }
                                Ok(dispatch(app, req.map(Body::new), conn).await)
                            }
                        });
                        let _ = hyper::server::conn::http1::Builder::new().keep_alive(true).serve_connection(io, svc).with_upgrades().await;
                    });
                }
                Some(_) = conns.join_next(), if !conns.is_empty() => {}
                _ = token.cancelled() => break,
            }
        }
        // stop sharing / shutdown: cut every connection
        conns.abort_all();
        while conns.join_next().await.is_some() {}
    });
    Listener { cancel, task }
}

/// Node's `clientError` reply (400, then the connection closes)
fn client_error() -> Response<Body> {
    let mut r = Response::new(Body::from(
        r#"{"error":"Bad Request","message":"Client Error","statusCode":400}"#,
    ));
    *r.status_mut() = StatusCode::BAD_REQUEST;
    add_header(&mut r, "content-type", "application/json");
    add_header(&mut r, "connection", "close");
    r
}

fn add_header(r: &mut Response<Body>, k: &'static str, v: &str) {
    if let Ok(v) = HeaderValue::from_str(v) {
        r.headers_mut().insert(k, v);
    }
}

async fn dispatch(app: Arc<App>, req: Request<Body>, conn: ConnInfo) -> Response<Body> {
    let ctx = &app.ctx;
    let (parts, body) = req.into_parts();
    let hv = |n: &str| {
        parts
            .headers
            .get(n)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    };
    let method = parts.method.as_str().to_string();
    let host = hv("host");
    let origin = hv("origin");
    // ── security (onRequest, before everything else) ──
    if !security::is_allowed_host(host.as_deref(), &ctx.security.allowed_hosts) {
        return json_error(
            421,
            "Unknown host. Add it to PEEK_ALLOWED_HOSTS to allow it.",
        );
    }
    let allowed = security::is_allowed_origin(
        origin.as_deref(),
        host.as_deref(),
        &ctx.security.allowed_origins,
        security::is_loopback_ip(&conn.ip),
    );
    let upgrade = hv("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    let safe = security::is_safe_method(&method);
    if !allowed && (upgrade || !safe) {
        return json_error(403, "Cross-origin request refused");
    }
    if origin.is_none() && !safe && hv("sec-fetch-site").as_deref() == Some("cross-site") {
        return json_error(403, "Cross-site request refused");
    }
    // ── CORS ──
    let mut vary = vec!["Origin"];
    let acao = if allowed { origin.clone() } else { None };
    if allowed && method == "OPTIONS" {
        let acrm = hv("access-control-request-method");
        if origin.is_none() || acrm.is_none() {
            let mut r = Response::new(Body::from("Invalid Preflight Request"));
            *r.status_mut() = StatusCode::BAD_REQUEST;
            add_header(&mut r, "content-type", "text/plain");
            add_header(&mut r, "vary", "Origin");
            if let Some(o) = &acao {
                add_header(&mut r, "access-control-allow-origin", o);
            }
            return r;
        }
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::NO_CONTENT;
        add_header(&mut r, "vary", "Origin, Access-Control-Request-Headers");
        if let Some(o) = &acao {
            add_header(&mut r, "access-control-allow-origin", o);
        }
        add_header(
            &mut r,
            "access-control-allow-methods",
            "GET, HEAD, PUT, POST, PATCH, DELETE, OPTIONS",
        );
        if let Some(h) = hv("access-control-request-headers") {
            add_header(&mut r, "access-control-allow-headers", &h);
        }
        add_header(&mut r, "content-length", "0");
        return r;
    }
    let path = parts.uri.path().to_string();
    // ── routing ──
    let decoded = percent_encoding::percent_decode_str(&path)
        .decode_utf8_lossy()
        .into_owned();
    let found = app.router.find(&method, &path);
    // The desktop LAN share is for the native phone app only (API + WebSocket): browsers reach the
    // web UI through the Docker server, so the LAN listener of a desktop app serves no web files.
    let web_allowed = !(ctx.cfg.desktop() && conn.lan);
    let web = if web_allowed && found.is_none() && (method == "GET" || method == "HEAD") {
        app.web_files.get(&decoded).cloned()
    } else {
        None
    };
    let is_route = found.is_some() || web.is_some() || method == "OPTIONS";
    let mut rate: Option<security::RateState> = None;
    if is_route {
        let internal = hv("x-peek-internal").is_some() && security::is_loopback_ip(&conn.ip);
        if !internal && !path.starts_with("/api/devices/reply/") {
            let (store, max) = match &found {
                Some((r, _)) if r.limit != security::Limit::Global => {
                    (format!("{} {}", r.method, r.pattern), r.limit.max())
                }
                _ => (String::new(), security::Limit::Global.max()),
            };
            rate = Some(ctx.rate.hit(&store, &conn.ip, max));
        }
    }
    let mut resp = if let Some(st) = rate.filter(|s| s.exceeded) {
        let mut r = Response::new(Body::from("{}"));
        *r.status_mut() = StatusCode::TOO_MANY_REQUESTS;
        add_header(&mut r, "content-type", "application/json; charset=utf-8");
        add_header(&mut r, "retry-after", &st.reset_s.to_string());
        r
    } else if let Some((route, params)) = found {
        let mut req = http::Req::new(parts, body, conn.clone());
        req.params = params;
        req.body_limit = route.body_limit.unwrap_or(32 * 1024 * 1024);
        let pattern = route.pattern.clone();
        let handler = route.handler.clone();
        let scope = route.scope;
        let r: error::ApiResult<Response<Body>> = async {
            req.parse_body(scope).await?;
            devices::device_access(ctx, &req, &pattern)?;
            handler(ctx.clone(), req).await
        }
        .await;
        match r {
            Ok(r) => r,
            Err(e) => {
                if e.status >= 500 && ctx.cfg.logger {
                    tracing::error!("{} {} → {}", method, path, e.message);
                }
                axum::response::IntoResponse::into_response(e)
            }
        }
    } else if let Some(f) = web {
        match http::send_file(
            &parts.headers,
            &parts.method,
            &f,
            http::SendOpts {
                cache_control: true,
                preset_cache_control: None,
            },
        )
        .await
        {
            Some(r) => r,
            None => json_error(404, "Not found"),
        }
    } else if let (Some(w), "GET", false, true) = (
        &app.web_dir,
        method.as_str(),
        path.starts_with("/api/"),
        web_allowed,
    ) {
        match http::send_file(
            &parts.headers,
            &parts.method,
            &w.join("index.html"),
            http::SendOpts {
                cache_control: true,
                preset_cache_control: None,
            },
        )
        .await
        {
            Some(mut r) => {
                add_header(&mut r, "content-type", "text/html; charset=utf-8");
                add_header(&mut r, "cache-control", "no-cache");
                r
            }
            None => json_error(404, "Not found"),
        }
    } else {
        json_error(404, "Not found")
    };
    // ── reply headers: rate limit, CORS ──
    if let Some(st) = rate {
        add_header(&mut resp, "x-ratelimit-limit", &st.max.to_string());
        add_header(
            &mut resp,
            "x-ratelimit-remaining",
            &st.remaining.to_string(),
        );
        add_header(&mut resp, "x-ratelimit-reset", &st.reset_s.to_string());
    }
    if let Some(existing) = resp
        .headers()
        .get(header::VARY)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
    {
        if !existing
            .split(',')
            .any(|v| v.trim().eq_ignore_ascii_case("origin"))
        {
            vary.insert(0, "");
            let v = format!("{existing}, Origin");
            add_header(&mut resp, "vary", &v);
        }
    } else {
        add_header(&mut resp, "vary", &vary.join(", "));
    }
    if let Some(o) = &acao {
        add_header(&mut resp, "access-control-allow-origin", o);
    }
    resp
}
