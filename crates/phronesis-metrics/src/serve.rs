//! Minimal HTTP endpoint exposing [`crate::scrape`].
//!
//! One route, no framework: hyper 1.x directly over a tokio listener. The
//! endpoint is loopback-only unless explicitly overridden, because the
//! exposition text carries project-internal rule ids.

use crate::{Error, Live, families};
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

/// OpenMetrics content type. Prometheus negotiates this; anything else makes
/// it fall back to the legacy text format and drop the `_created` series.
const CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub addr: SocketAddr,
    pub project_root: PathBuf,
    pub options: families::Options,
}

impl ServeConfig {
    pub fn new(addr: SocketAddr, project_root: PathBuf) -> Self {
        Self {
            addr,
            project_root,
            options: families::Options::default(),
        }
    }
}

/// Bind the listener, enforcing the loopback guard *before* the socket opens.
///
/// Returns the real local address, which matters when binding port 0.
pub async fn bind(cfg: &ServeConfig) -> Result<(TcpListener, SocketAddr), Error> {
    if !cfg.addr.ip().is_loopback() {
        return Err(Error::NonLoopbackBind(cfg.addr));
    }
    let listener = TcpListener::bind(cfg.addr).await?;
    let local = listener.local_addr()?;
    Ok((listener, local))
}

/// Bind and serve until the future is dropped.
pub async fn serve(cfg: ServeConfig, live: Option<Live>) -> Result<(), Error> {
    let (listener, _) = bind(&cfg).await?;
    serve_on(listener, cfg, live).await
}

/// Serve on an already-bound listener. Split out so tests can bind port 0 and
/// learn the assigned port before traffic starts.
pub async fn serve_on(
    listener: TcpListener,
    cfg: ServeConfig,
    live: Option<Live>,
) -> Result<(), Error> {
    let cfg = Arc::new(cfg);
    let live = live.map(Arc::new);
    loop {
        // A failed accept (fd exhaustion, a peer that vanished) must not take
        // the exporter down — metrics are the thing you want *most* when the
        // host is unhealthy.
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let cfg = Arc::clone(&cfg);
        let live = live.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                let cfg = Arc::clone(&cfg);
                let live = live.clone();
                async move { Ok::<_, std::convert::Infallible>(route(req, &cfg, live.as_deref())) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });
    }
}

fn route(
    req: Request<hyper::body::Incoming>,
    cfg: &ServeConfig,
    live: Option<&Live>,
) -> Response<Full<Bytes>> {
    match req.uri().path() {
        "/metrics" => match crate::scrape(&cfg.project_root, &cfg.options, live) {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .header("content-type", CONTENT_TYPE)
                .body(Full::new(Bytes::from(body)))
                .expect("static response builds"),
            Err(e) => text(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}\n")),
        },
        "/" => text(
            StatusCode::OK,
            "phronesis metrics: see /metrics\n".to_string(),
        ),
        _ => text(StatusCode::NOT_FOUND, "not found\n".to_string()),
    }
}

fn text(status: StatusCode, body: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(body)))
        .expect("static response builds")
}
