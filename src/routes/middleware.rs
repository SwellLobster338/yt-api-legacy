use actix_web::{
    body::BoxBody,
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    Error, HttpResponse,
};
use futures_util::future::LocalBoxFuture;
use std::future::{ready, Ready};
use std::task::{Context, Poll};

pub struct IpBlocker;

impl<S, B> Transform<S, ServiceRequest> for IpBlocker
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: actix_web::body::MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type InitError = ();
    type Transform = IpBlockerMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(IpBlockerMiddleware { service }))
    }
}

pub struct IpBlockerMiddleware<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for IpBlockerMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: actix_web::body::MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        // Get client IP
        let peer_addr = req
            .connection_info()
            .peer_addr()
            .map(|addr| {
                // Extract IP without port
                addr.split(':').next().unwrap_or(addr).to_string()
            })
            .unwrap_or_else(|| "unknown".to_string());

        // Check if IP is blocked
        if crate::ip_blocker::is_ip_blocked(&peer_addr) {
            log::info!("Blocked request from IP: {}", peer_addr);
            
            let (request, _payload) = req.into_parts();
            let response = HttpResponse::Forbidden()
                .content_type("text/plain")
                .body("Access Denied");
            
            let service_response = ServiceResponse::new(request, response);
            return Box::pin(async move {
                Ok(service_response.map_into_boxed_body())
            });
        }

        // IP is not blocked, continue with the service
        let fut = self.service.call(req);

        Box::pin(async move {
            let res = fut.await?;
            Ok(res.map_into_boxed_body())
        })
    }
}
