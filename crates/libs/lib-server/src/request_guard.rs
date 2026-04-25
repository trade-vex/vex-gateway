use actix_web::{
    Error, HttpResponse,
    body::{BoxBody, MessageBody},
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    http::StatusCode,
};
use futures_util::future::{LocalBoxFuture, Ready, ready};
use serde_json::json;
use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const AUTH_LIMIT: usize = 128;
const MUTATION_LIMIT: usize = 128;
const READ_LIMIT: usize = 2_048;
const GENERAL_LIMIT: usize = 4_096;

#[derive(Clone)]
pub struct RequestLimits {
    inner: Arc<RequestLimitsInner>,
}

struct RequestLimitsInner {
    timeout: Duration,
    auth: Arc<Semaphore>,
    mutation: Arc<Semaphore>,
    read: Arc<Semaphore>,
    general: Arc<Semaphore>,
}

impl RequestLimits {
    pub fn default_limits() -> Self {
        Self {
            inner: Arc::new(RequestLimitsInner {
                timeout: REQUEST_TIMEOUT,
                auth: Arc::new(Semaphore::new(AUTH_LIMIT)),
                mutation: Arc::new(Semaphore::new(MUTATION_LIMIT)),
                read: Arc::new(Semaphore::new(READ_LIMIT)),
                general: Arc::new(Semaphore::new(GENERAL_LIMIT)),
            }),
        }
    }

    pub fn timeout(&self) -> Duration {
        self.inner.timeout
    }

    fn semaphore_for(&self, class: RouteClass) -> Option<Arc<Semaphore>> {
        match class {
            RouteClass::Health => None,
            RouteClass::Auth => Some(Arc::clone(&self.inner.auth)),
            RouteClass::Mutation => Some(Arc::clone(&self.inner.mutation)),
            RouteClass::Read => Some(Arc::clone(&self.inner.read)),
            RouteClass::General => Some(Arc::clone(&self.inner.general)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RouteClass {
    Health,
    Auth,
    Mutation,
    Read,
    General,
}

fn classify_request(req: &ServiceRequest) -> RouteClass {
    let path = req.path();
    let method = req.method().as_str();

    if path == "/api/health" {
        return RouteClass::Health;
    }

    if path.contains("/login")
        || path.contains("/signup")
        || path.contains("/reset-password")
        || path.contains("/change-password")
        || path.contains("/verify-email")
        || path.contains("/forgot-password")
        || path.contains("/oauth/")
    {
        return RouteClass::Auth;
    }

    if matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
        && (path.contains("/order")
            || path.contains("/cancel")
            || path.contains("/favorites")
            || path.contains("/api-key"))
    {
        return RouteClass::Mutation;
    }

    if matches!(method, "GET" | "HEAD") {
        return RouteClass::Read;
    }

    RouteClass::General
}

#[derive(Clone)]
pub struct RequestGuard {
    limits: RequestLimits,
}

impl RequestGuard {
    pub fn new(limits: RequestLimits) -> Self {
        Self { limits }
    }
}

impl<S, B> Transform<S, ServiceRequest> for RequestGuard
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type Transform = RequestGuardService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(RequestGuardService {
            service: Rc::new(service),
            limits: self.limits.clone(),
        }))
    }
}

pub struct RequestGuardService<S> {
    service: Rc<S>,
    limits: RequestLimits,
}

impl<S, B> Service<ServiceRequest> for RequestGuardService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = Rc::clone(&self.service);
        let limits = self.limits.clone();
        let class = classify_request(&req);
        let method = req.method().clone();
        let path = req.path().to_string();
        let started = Instant::now();

        Box::pin(async move {
            debug!(request_class = ?class, method = %method, path = %path, "request_start");

            let _permit = match limits.semaphore_for(class) {
                Some(semaphore) => match semaphore.try_acquire_owned() {
                    Ok(permit) => Some(permit),
                    Err(_) => {
                        warn!(
                            request_class = ?class,
                            method = %method,
                            path = %path,
                            "request_rejected_overloaded"
                        );
                        return Ok(req
                            .into_response(error_response(
                                StatusCode::TOO_MANY_REQUESTS,
                                "REQUEST_OVERLOADED",
                                "Server is busy, retry shortly",
                            ))
                            .map_into_boxed_body());
                    }
                },
                None => None,
            };

            match tokio::time::timeout(limits.timeout(), service.call(req)).await {
                Ok(Ok(response)) => {
                    debug!(
                        request_class = ?class,
                        method = %method,
                        path = %path,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "request_finish"
                    );
                    Ok(response.map_into_boxed_body())
                }
                Ok(Err(error)) => Err(error),
                Err(_) => {
                    warn!(
                        request_class = ?class,
                        method = %method,
                        path = %path,
                        timeout_ms = limits.timeout().as_millis() as u64,
                        "request_timed_out"
                    );
                    Err(actix_web::error::ErrorGatewayTimeout(
                        "Request exceeded server deadline",
                    ))
                }
            }
        })
    }
}

fn error_response(status: StatusCode, code: &'static str, message: &'static str) -> HttpResponse {
    HttpResponse::build(status)
        .insert_header(("Cache-Control", "no-store"))
        .json(json!({
            "error": {
                "code": code,
                "message": message
            }
        }))
}
