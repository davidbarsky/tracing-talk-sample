use eyre::Report;
use http::{Request, Response};
use hyper::{server::conn::Http, Body};
use std::{
    net::SocketAddr,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    stream::StreamExt,
};
use tower::{layer::Layer, service_fn, Service, ServiceBuilder};
use tracing::{error, field, info, span, Instrument as _, Level, Span};
use tracing_subscriber::{
    filter::LevelFilter,
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    registry::Registry,
    EnvFilter,
};

struct TracingLayer;

impl<S> Layer<S> for TracingLayer {
    type Service = TracingService<S>;

    fn layer(&self, service: S) -> Self::Service {
        TracingService { service }
    }
}

struct TracingService<S> {
    service: S,
}

impl<T, S> Service<Request<T>> for TracingService<S>
where
    S: Service<Request<T>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&mut self, req: Request<T>) -> Self::Future {
        let span = span!(
            Level::INFO,
            "request",
            http.req.method = %req.method(),
            http.req.uri = %req.uri();
            http.req.version = ?req.version(),
        );

        self.service.call(req).instrument(span)
    }
}

async fn hello(_: Request<Body>) -> Result<Response<Body>, Report> {
    Ok(Response::new(Body::from("hello")))
}

async fn handle<I>(io: I) -> Result<(), hyper::error::Error>
where
    I: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    let span = span!(Level::INFO, "handler");
    let service = ServiceBuilder::new()
        .layer(TracingLayer)
        .service(service_fn(hello));

    let conn = Http::new().serve_connection(io, service).instrument(span);
    if let Err(e) = conn.await {
        error!(message = "Got error serving connection", e = %e);
        return Err(e);
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Report> {
    let filter = EnvFilter::default()
        .add_directive(LevelFilter::INFO.into())
        .add_directive("tokio::task=trace".parse()?);

    let fmt_layer = fmt::Layer::default().with_span_events(FmtSpan::FULL);
    let subscriber = Registry::default().with(fmt_layer).with(filter);
    tracing::subscriber::set_global_default(subscriber)?;

    let addr: SocketAddr = ([127, 0, 0, 1], 3000).into();
    let mut listener = TcpListener::bind(&addr).await?;
    info!(target: "http-server", message = "server listening on:", addr = ?addr);

    while let Some(stream) = listener.incoming().next().await {
        match stream {
            Ok(stream) => tokio::spawn(handle(stream)).await??,
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}
