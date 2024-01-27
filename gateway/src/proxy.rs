use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::headers::{HeaderMapExt, Host};
use axum::response::{IntoResponse, Response};
use axum_server::accept::DefaultAcceptor;
use axum_server::tls_rustls::RustlsAcceptor;
use fqdn::{fqdn, FQDN};
use futures::future::{ready, Ready};
use futures::prelude::*;
use hyper::body::{Body, HttpBody};
use hyper::client::connect::dns::GaiResolver;
use hyper::client::HttpConnector;
use hyper::server::conn::AddrStream;
use hyper::{Client, Request};
use hyper_reverse_proxy::ReverseProxy;
use once_cell::sync::Lazy;
use opentelemetry::global;
use opentelemetry_http::HeaderInjector;
use shuttle_common::backends::headers::XShuttleProject;
use shuttle_common::models::error::InvalidProjectName;
use tokio::sync::mpsc::Sender;
use tower::{Service, ServiceBuilder};
use tower_sanitize_path::SanitizePath;
use tracing::{debug_span, error, field, trace};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::acme::{AcmeClient, ChallengeResponderLayer, CustomDomain};
use crate::service::GatewayService;
use crate::task::BoxedTask;
use crate::{Error, ErrorKind};

static PROXY_CLIENT: Lazy<ReverseProxy<HttpConnector<GaiResolver>>> =
    Lazy::new(|| ReverseProxy::new(Client::new()));

pub trait AsResponderTo<R> {
    fn as_responder_to(&self, req: R) -> Self;

    fn into_make_service(self) -> ResponderMakeService<Self>
    where
        Self: Sized,
    {
        ResponderMakeService { inner: self }
    }
}

pub struct ResponderMakeService<S> {
    inner: S,
}

impl<'r, S> Service<&'r AddrStream> for ResponderMakeService<S>
where
    S: AsResponderTo<&'r AddrStream>,
{
    type Response = S;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: &'r AddrStream) -> Self::Future {
        ready(Ok(self.inner.as_responder_to(req)))
    }
}

#[derive(Clone)]
pub struct UserProxy {
    gateway: Arc<GatewayService>,
    task_sender: Sender<BoxedTask>,
    remote_addr: SocketAddr,
    public: FQDN,
}

impl<'r> AsResponderTo<&'r AddrStream> for UserProxy {
    fn as_responder_to(&self, addr_stream: &'r AddrStream) -> Self {
        let mut responder = self.clone();
        responder.remote_addr = addr_stream.remote_addr();
        responder
    }
}

impl<S, R> AsResponderTo<R> for SanitizePath<S>
where
    S: AsResponderTo<R> + Clone,
{
    fn as_responder_to(&self, req: R) -> Self {
        let responder = self.clone();
        responder.inner().as_responder_to(req);

        responder
    }
}

impl UserProxy {
    async fn proxy(
        self,
        task_sender: Sender<BoxedTask>,
        mut req: Request<Body>,
    ) -> Result<Response, Error> {
        let span = debug_span!("proxy", http.method = %req.method(), http.host = field::Empty, http.uri = %req.uri(), http.status_code = field::Empty, shuttle.project.name = field::Empty);
        trace!(?req, "serving proxy request");

        let fqdn = req
            .headers()
            .typed_get::<Host>()
            .map(|host| fqdn!(host.hostname()))
            .ok_or_else(|| Error::from_kind(ErrorKind::BadHost))?;

        span.record("http.host", fqdn.to_string());

        let project_name = if fqdn.is_subdomain_of(&self.public)
            && fqdn.depth() - self.public.depth() == 1
        {
            fqdn.labels()
                .next()
                .unwrap()
                .to_owned()
                .parse()
                .map_err(|_| Error::from_kind(ErrorKind::InvalidProjectName(InvalidProjectName)))?
        } else if let Ok(CustomDomain { project_name, .. }) =
            self.gateway.project_details_for_custom_domain(&fqdn).await
        {
            project_name
        } else {
            return Err(Error::from_kind(ErrorKind::CustomDomainNotFound));
        };

        req.headers_mut()
            .typed_insert(XShuttleProject(project_name.to_string()));

        let project = self
            .gateway
            .find_or_start_project(&project_name, task_sender)
            .await?;

        // Record current project for tracing purposes
        span.record("shuttle.project.name", &project_name.to_string());

        let target_ip = project
            .state
            .target_ip()?
            .ok_or_else(|| Error::from_kind(ErrorKind::ProjectNotReady))?;

        let target_url = format!("http://{}:{}", target_ip, 8000);

        let cx = span.context();

        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&cx, &mut HeaderInjector(req.headers_mut()))
        });

        let proxy = PROXY_CLIENT
            .call(self.remote_addr.ip(), &target_url, req)
            .await
            .map_err(|_| Error::from_kind(ErrorKind::ProjectUnavailable))?;

        let (parts, body) = proxy.into_parts();
        let body = <Body as HttpBody>::map_err(body, axum::Error::new).boxed_unsync();

        span.record("http.status_code", parts.status.as_u16());

        Ok(Response::from_parts(parts, body))
    }
}

impl Service<Request<Body>> for UserProxy {
    type Response = Response;
    type Error = Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let task_sender = self.task_sender.clone();
        self.clone()
            .proxy(task_sender, req)
            .or_else(|err: Error| future::ready(Ok(err.into_response())))
            .boxed()
    }
}

#[derive(Clone)]
pub struct Bouncer {
    gateway: Arc<GatewayService>,
    public: FQDN,
}

impl<'r> AsResponderTo<&'r AddrStream> for Bouncer {
    fn as_responder_to(&self, _req: &'r AddrStream) -> Self {
        self.clone()
    }
}

impl Bouncer {
    async fn bounce(self, req: Request<Body>) -> Result<Response, Error> {
        let mut resp = Response::builder();

        let host = req.headers().typed_get::<Host>().unwrap();
        let hostname = host.hostname();
        let fqdn = fqdn!(hostname);

        let path = req.uri();

        if fqdn.is_subdomain_of(&self.public)
            || self
                .gateway
                .project_details_for_custom_domain(&fqdn)
                .await
                .is_ok()
        {
            resp = resp
                .status(301)
                .header("Location", format!("https://{hostname}{path}"));
        } else {
            resp = resp.status(404);
        }

        let body = <Body as HttpBody>::map_err(Body::empty(), axum::Error::new).boxed_unsync();

        Ok(resp.body(body).unwrap())
    }
}

impl Service<Request<Body>> for Bouncer {
    type Response = Response;
    type Error = Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        self.clone().bounce(req).boxed()
    }
}

#[derive(Default)]
pub struct UserServiceBuilder {
    service: Option<Arc<GatewayService>>,
    task_sender: Option<Sender<BoxedTask>>,
    acme: Option<AcmeClient>,
    tls_acceptor: Option<RustlsAcceptor<DefaultAcceptor>>,
    bouncer_binds_to: Option<SocketAddr>,
    user_binds_to: Option<SocketAddr>,
    public: Option<FQDN>,
}

impl UserServiceBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_public(mut self, public: FQDN) -> Self {
        self.public = Some(public);
        self
    }

    pub fn with_service(mut self, service: Arc<GatewayService>) -> Self {
        self.service = Some(service);
        self
    }

    pub fn with_task_sender(mut self, task_sender: Sender<BoxedTask>) -> Self {
        self.task_sender = Some(task_sender);
        self
    }

    pub fn with_bouncer(mut self, bound_to: SocketAddr) -> Self {
        self.bouncer_binds_to = Some(bound_to);
        self
    }

    pub fn with_user_proxy_binding_to(mut self, bound_to: SocketAddr) -> Self {
        self.user_binds_to = Some(bound_to);
        self
    }

    pub fn with_acme(mut self, acme: AcmeClient) -> Self {
        self.acme = Some(acme);
        self
    }

    pub fn with_tls(mut self, acceptor: RustlsAcceptor<DefaultAcceptor>) -> Self {
        self.tls_acceptor = Some(acceptor);
        self
    }

    pub fn serve(self) -> impl Future<Output = Result<(), io::Error>> {
        let service = self.service.expect("a GatewayService is required");
        let task_sender = self.task_sender.expect("a task sender is required");
        let public = self.public.expect("a public FQDN is required");
        let user_binds_to = self
            .user_binds_to
            .expect("a socket address to bind to is required");

        let user_proxy = SanitizePath::sanitize_paths(UserProxy {
            gateway: service.clone(),
            task_sender,
            remote_addr: "127.0.0.1:80".parse().unwrap(),
            public: public.clone(),
        })
        .into_make_service();

        let bouncer = self.bouncer_binds_to.as_ref().map(|_| Bouncer {
            gateway: service.clone(),
            public: public.clone(),
        });

        let mut futs = Vec::new();
        if let Some(tls_acceptor) = self.tls_acceptor {
            // TLS is enabled
            let bouncer = bouncer.expect("TLS cannot be enabled without a bouncer");
            let bouncer_binds_to = self.bouncer_binds_to.unwrap();

            let acme = self
                .acme
                .expect("TLS cannot be enabled without an ACME client");

            let bouncer = ServiceBuilder::new()
                .layer(ChallengeResponderLayer::new(acme))
                .service(bouncer);

            let bouncer = axum_server::Server::bind(bouncer_binds_to)
                .serve(bouncer.into_make_service())
                .map(|handle| ("bouncer (with challenge responder)", handle))
                .boxed();

            futs.push(bouncer);

            let user_with_tls = axum_server::Server::bind(user_binds_to)
                .acceptor(tls_acceptor)
                .serve(user_proxy)
                .map(|handle| ("user proxy (with TLS)", handle))
                .boxed();
            futs.push(user_with_tls);
        } else {
            if let Some(bouncer) = bouncer {
                // bouncer is enabled
                let bouncer_binds_to = self.bouncer_binds_to.unwrap();
                let bouncer = axum_server::Server::bind(bouncer_binds_to)
                    .serve(bouncer.into_make_service())
                    .map(|handle| ("bouncer (without challenge responder)", handle))
                    .boxed();
                futs.push(bouncer);
            }

            let user_without_tls = axum_server::Server::bind(user_binds_to)
                .serve(user_proxy)
                .map(|handle| ("user proxy (no TLS)", handle))
                .boxed();
            futs.push(user_without_tls);
        }

        future::select_all(futs).map(|((name, resolved), _, _)| {
            error!(service = %name, "exited early");
            resolved
        })
    }
}
