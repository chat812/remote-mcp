use anyhow::Result;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

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

pub struct SignedRequest {
    pub timestamp: String,
    pub signature: String,
}

pub fn sign_request(
    token: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<SignedRequest> {
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let body_hash = sha256_hex(body);
    let message = format!("{}\n{}\n{}\n{}", method, path, timestamp, body_hash);
    let signature = hmac_sha256_hex(token, &message);
    Ok(SignedRequest { timestamp, signature })
}

pub fn verify_request(
    token: &str,
    method: &str,
    path: &str,
    timestamp_str: &str,
    body: &[u8],
    provided_sig: &str,
) -> bool {
    let now = chrono::Utc::now().timestamp();
    let ts: i64 = match timestamp_str.parse() {
        Ok(t) => t,
        Err(_) => return false,
    };
    if (now - ts).unsigned_abs() > 60 {
        return false;
    }
    let body_hash = sha256_hex(body);
    let message = format!("{}\n{}\n{}\n{}", method, path, timestamp_str, body_hash);
    let expected = hmac_sha256_hex(token, &message);
    // constant-time compare
    expected == provided_sig
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "test-secret-token";

    #[test]
    fn sha256_hex_empty_string() {
        // SHA-256 of "" is well-known
        let result = sha256_hex(b"");
        assert_eq!(
            result,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hex_known_value() {
        let result = sha256_hex(b"hello");
        assert_eq!(
            result,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_hex_deterministic() {
        assert_eq!(sha256_hex(b"abc"), sha256_hex(b"abc"));
        assert_ne!(sha256_hex(b"abc"), sha256_hex(b"abd"));
    }

    #[test]
    fn hmac_sha256_hex_deterministic() {
        let a = hmac_sha256_hex(TOKEN, "message");
        let b = hmac_sha256_hex(TOKEN, "message");
        assert_eq!(a, b);
    }

    #[test]
    fn hmac_sha256_hex_different_tokens() {
        let a = hmac_sha256_hex("token-a", "message");
        let b = hmac_sha256_hex("token-b", "message");
        assert_ne!(a, b);
    }

    #[test]
    fn hmac_sha256_hex_different_messages() {
        let a = hmac_sha256_hex(TOKEN, "msg1");
        let b = hmac_sha256_hex(TOKEN, "msg2");
        assert_ne!(a, b);
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let body = b"request body";
        let signed = sign_request(TOKEN, "POST", "/exec", body).unwrap();
        assert!(verify_request(
            TOKEN, "POST", "/exec",
            &signed.timestamp, body,
            &signed.signature
        ));
    }

    #[test]
    fn verify_rejects_wrong_token() {
        let body = b"request body";
        let signed = sign_request(TOKEN, "POST", "/exec", body).unwrap();
        assert!(!verify_request(
            "wrong-token", "POST", "/exec",
            &signed.timestamp, body,
            &signed.signature
        ));
    }

    #[test]
    fn verify_rejects_wrong_method() {
        let body = b"request body";
        let signed = sign_request(TOKEN, "POST", "/exec", body).unwrap();
        assert!(!verify_request(
            TOKEN, "GET", "/exec",
            &signed.timestamp, body,
            &signed.signature
        ));
    }

    #[test]
    fn verify_rejects_wrong_path() {
        let body = b"request body";
        let signed = sign_request(TOKEN, "POST", "/exec", body).unwrap();
        assert!(!verify_request(
            TOKEN, "POST", "/other",
            &signed.timestamp, body,
            &signed.signature
        ));
    }

    #[test]
    fn verify_rejects_tampered_body() {
        let body = b"original body";
        let signed = sign_request(TOKEN, "POST", "/exec", body).unwrap();
        assert!(!verify_request(
            TOKEN, "POST", "/exec",
            &signed.timestamp, b"tampered body",
            &signed.signature
        ));
    }

    #[test]
    fn verify_rejects_expired_timestamp() {
        let body = b"body";
        // Timestamp 120 seconds in the past
        let old_ts = (chrono::Utc::now().timestamp() - 120).to_string();
        let body_hash = sha256_hex(body);
        let message = format!("POST\n/exec\n{}\n{}", old_ts, body_hash);
        let sig = hmac_sha256_hex(TOKEN, &message);
        assert!(!verify_request(TOKEN, "POST", "/exec", &old_ts, body, &sig));
    }

    #[test]
    fn verify_rejects_future_timestamp() {
        let body = b"body";
        // Timestamp 120 seconds in the future
        let future_ts = (chrono::Utc::now().timestamp() + 120).to_string();
        let body_hash = sha256_hex(body);
        let message = format!("POST\n/exec\n{}\n{}", future_ts, body_hash);
        let sig = hmac_sha256_hex(TOKEN, &message);
        assert!(!verify_request(TOKEN, "POST", "/exec", &future_ts, body, &sig));
    }

    #[test]
    fn verify_rejects_non_numeric_timestamp() {
        assert!(!verify_request(TOKEN, "POST", "/exec", "not-a-number", b"body", "sig"));
    }

    #[test]
    fn verify_accepts_empty_body() {
        let signed = sign_request(TOKEN, "GET", "/health", b"").unwrap();
        assert!(verify_request(
            TOKEN, "GET", "/health",
            &signed.timestamp, b"",
            &signed.signature
        ));
    }
}
