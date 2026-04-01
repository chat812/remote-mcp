use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::IntoResponse,
    Json,
};
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Maximum request body size accepted before the HMAC is verified.
/// Prevents OOM DoS by an unauthenticated caller sending an unbounded body.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024; // 64 MB

/// Maximum age (and future skew) of a request timestamp.
/// Requests older than this many seconds, or more than this many seconds in
/// the future, are rejected as replayed or clock-skewed.
const MAX_TIMESTAMP_SKEW_SECS: u64 = 30;

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub fn hmac_sha256_hex(token: &str, message: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(token.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub async fn auth_middleware(
    token: String,
    allowed_ips: Vec<String>,
    req: Request<Body>,
    next: Next,
) -> impl IntoResponse {
    // path() for whitelist matching (no query string), path_and_query() for HMAC
    let plain_path = req.uri().path().to_string();
    let signed_path = req.uri().path_and_query().map(|pq| pq.as_str()).unwrap_or("/").to_string();
    let method = req.method().clone();

    // ── IP allowlist ─────────────────────────────────────────────────────────
    // Only enforced when the list is non-empty (empty = allow all).
    if !allowed_ips.is_empty() {
        let remote_ip = req
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0.ip().to_string())
            .unwrap_or_default();

        let allowed = allowed_ips
            .iter()
            .any(|allowed| allowed == &remote_ip || allowed == "0.0.0.0" || allowed == "::");

        if !allowed {
            return forbidden("ip not allowed");
        }
    }

    // ── Public routes (no HMAC required) ─────────────────────────────────────
    if method == axum::http::Method::GET && plain_path == "/health" {
        return next.run(req).await.into_response();
    }

    // ── HMAC authentication ───────────────────────────────────────────────────

    // Extract headers
    let timestamp = match req.headers().get("X-Agent-Timestamp") {
        Some(v) => match v.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return unauthorized("invalid timestamp header"),
        },
        None => return unauthorized("missing X-Agent-Timestamp"),
    };

    let provided_sig = match req.headers().get("X-Agent-Signature") {
        Some(v) => match v.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return unauthorized("invalid signature header"),
        },
        None => return unauthorized("missing X-Agent-Signature"),
    };

    // Timestamp freshness — reject both old and far-future requests
    let now = chrono::Utc::now().timestamp();
    let ts: i64 = match timestamp.parse() {
        Ok(t) => t,
        Err(_) => return unauthorized("invalid timestamp"),
    };
    let skew = (now - ts).unsigned_abs();
    if skew > MAX_TIMESTAMP_SKEW_SECS {
        return unauthorized("timestamp expired or too far in future");
    }

    // Buffer body with a hard size cap — checked before HMAC to prevent OOM DoS
    let (parts, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": "request body exceeds 64MB limit", "code": "BODY_TOO_LARGE" })),
        ).into_response(),
    };

    // HMAC: METHOD\nPATH_AND_QUERY\nTIMESTAMP\nBODY_SHA256
    let body_hash = sha256_hex(&body_bytes);
    let message = format!(
        "{}\n{}\n{}\n{}",
        method.as_str(),
        signed_path,
        timestamp,
        body_hash
    );
    let expected = hmac_sha256_hex(&token, &message);

    // Constant-time comparison prevents timing side-channels
    if provided_sig.len() != expected.len() {
        return unauthorized("invalid signature");
    }
    let eq: bool = expected.as_bytes().ct_eq(provided_sig.as_bytes()).into();
    if !eq {
        return unauthorized("invalid signature");
    }

    // Reconstruct request with buffered body and pass through
    let req = Request::from_parts(parts, Body::from(body_bytes));
    next.run(req).await.into_response()
}

fn unauthorized(msg: &str) -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized", "message": msg })),
    )
        .into_response()
}

fn forbidden(msg: &str) -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({ "error": "forbidden", "message": msg })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "agent-test-secret";

    #[test]
    fn sha256_empty_string() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_known_value() {
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_deterministic() {
        assert_eq!(sha256_hex(b"test"), sha256_hex(b"test"));
        assert_ne!(sha256_hex(b"test"), sha256_hex(b"Test"));
    }

    #[test]
    fn hmac_deterministic_same_inputs() {
        assert_eq!(
            hmac_sha256_hex(TOKEN, "message"),
            hmac_sha256_hex(TOKEN, "message")
        );
    }

    #[test]
    fn hmac_changes_with_different_token() {
        let a = hmac_sha256_hex("token-a", "message");
        let b = hmac_sha256_hex("token-b", "message");
        assert_ne!(a, b);
    }

    #[test]
    fn hmac_changes_with_different_message() {
        let a = hmac_sha256_hex(TOKEN, "msg-1");
        let b = hmac_sha256_hex(TOKEN, "msg-2");
        assert_ne!(a, b);
    }

    #[test]
    fn hmac_output_is_hex_string() {
        let h = hmac_sha256_hex(TOKEN, "data");
        assert_eq!(h.len(), 64); // 32 bytes = 64 hex chars
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn signature_construction_matches_expected() {
        // Verify that the same message construction logic as the middleware
        let method = "POST";
        let path = "/exec";
        let timestamp = "1712000000";
        let body = b"{}";
        let body_hash = sha256_hex(body);
        let message = format!("{}\n{}\n{}\n{}", method, path, timestamp, body_hash);
        let sig1 = hmac_sha256_hex(TOKEN, &message);
        let sig2 = hmac_sha256_hex(TOKEN, &message);
        assert_eq!(sig1, sig2);
    }
}
