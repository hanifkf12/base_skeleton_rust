use std::net::{IpAddr, SocketAddr};

use axum::{
    Json,
    body::Body,
    extract::ConnectInfo,
    http::{Request, Response, StatusCode, header},
    response::IntoResponse,
};
use ipnet::IpNet;
use serde_json::json;
use tower_governor::{GovernorError, key_extractor::KeyExtractor};

#[derive(Clone, Copy, Debug)]
pub(crate) struct RateLimitRejection;

#[derive(Clone, Debug)]
pub struct TrustedProxyIpKeyExtractor {
    trusted_proxy_cidrs: Vec<IpNet>,
}

impl TrustedProxyIpKeyExtractor {
    pub fn new(trusted_proxy_cidrs: Vec<IpNet>) -> Self {
        Self {
            trusted_proxy_cidrs,
        }
    }
    fn is_trusted(&self, ip: IpAddr) -> bool {
        self.trusted_proxy_cidrs
            .iter()
            .any(|network| network.contains(&ip))
    }
}

impl KeyExtractor for TrustedProxyIpKeyExtractor {
    type Key = IpAddr;

    fn extract<T>(&self, request: &Request<T>) -> Result<Self::Key, GovernorError> {
        let peer = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|connect_info| connect_info.0.ip())
            .or_else(|| request.extensions().get::<SocketAddr>().map(SocketAddr::ip))
            // Axum always supplies ConnectInfo in production; this fallback keeps
            // Router::oneshot tests deterministic.
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));

        if !self.is_trusted(peer) {
            return Ok(peer);
        }

        let Some(value) = request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
        else {
            return Ok(peer);
        };
        for entry in value.rsplit(',').map(str::trim) {
            let Ok(ip) = entry.parse::<IpAddr>() else {
                break;
            };
            if !self.is_trusted(ip) {
                return Ok(ip);
            }
        }
        Ok(peer)
    }
}

pub fn error_response(error: GovernorError) -> Response<Body> {
    match error {
        GovernorError::TooManyRequests { wait_time, .. } => {
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": {
                        "code": "rate_limit_exceeded",
                        "message": "request rate limit exceeded"
                    }
                })),
            )
                .into_response();
            response.extensions_mut().insert(RateLimitRejection);
            response.headers_mut().insert(
                header::RETRY_AFTER,
                wait_time
                    .max(1)
                    .to_string()
                    .parse()
                    .expect("retry duration is a valid header value"),
            );
            response
        }
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": {
                    "code": "rate_limit_unavailable",
                    "message": "request rate limiter is unavailable"
                }
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(peer: &str, forwarded: Option<&str>) -> Request<()> {
        let mut request = Request::new(());
        request
            .extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().expect("valid peer")));
        if let Some(forwarded) = forwarded {
            request
                .headers_mut()
                .insert("x-forwarded-for", forwarded.parse().unwrap());
        }
        request
    }

    #[test]
    fn trusts_forwarded_chain_only_from_configured_proxy() {
        let extractor = TrustedProxyIpKeyExtractor::new(vec!["10.0.0.0/8".parse().unwrap()]);
        assert_eq!(
            extractor
                .extract(&request(
                    "10.1.2.3:1234",
                    Some("198.51.100.9, 203.0.113.7, 10.2.3.4")
                ))
                .unwrap(),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            extractor
                .extract(&request("192.0.2.3:1234", Some("203.0.113.7")))
                .unwrap(),
            "192.0.2.3".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            extractor
                .extract(&request("10.1.2.3:1234", Some("bad, 203.0.113.7")))
                .unwrap(),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            extractor
                .extract(&request("10.1.2.3:1234", Some("203.0.113.7, bad")))
                .unwrap(),
            "10.1.2.3".parse::<IpAddr>().unwrap()
        );
    }
}
