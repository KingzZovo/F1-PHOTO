use anyhow::Result;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Default access token lifetime in seconds (24h).
pub const DEFAULT_TTL_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    /// User UUID.
    pub sub: Uuid,
    pub username: String,
    /// `"admin"` or `"member"`.
    pub role: String,
    /// Issued at (unix seconds).
    pub iat: i64,
    /// Expiration (unix seconds).
    pub exp: i64,
}

/// Stateless HS256 codec. One instance per process.
pub struct JwtCodec {
    enc: EncodingKey,
    dec: DecodingKey,
    ttl_seconds: i64,
}

impl JwtCodec {
    pub fn new(secret: &str, ttl_seconds: i64) -> Self {
        Self {
            enc: EncodingKey::from_secret(secret.as_bytes()),
            dec: DecodingKey::from_secret(secret.as_bytes()),
            ttl_seconds,
        }
    }

    pub fn ttl_seconds(&self) -> i64 {
        self.ttl_seconds
    }

    /// Mint a new access token for a logged-in user.
    pub fn issue(&self, user_id: Uuid, username: &str, role: &str) -> Result<String> {
        let now = Utc::now();
        let claims = Claims {
            sub: user_id,
            username: username.into(),
            role: role.into(),
            iat: now.timestamp(),
            exp: (now + Duration::seconds(self.ttl_seconds)).timestamp(),
        };
        let token = encode(&Header::default(), &claims, &self.enc)?;
        Ok(token)
    }

    /// Validate a token and return its claims. 30s clock leeway for skew.
    pub fn verify(&self, token: &str) -> Result<Claims> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = 30;
        let data = decode::<Claims>(token, &self.dec, &validation)?;
        Ok(data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_then_verify() {
        let codec = JwtCodec::new("a".repeat(64).as_str(), 60);
        let uid = Uuid::new_v4();
        let tok = codec.issue(uid, "alice", "admin").unwrap();
        let c = codec.verify(&tok).unwrap();
        assert_eq!(c.sub, uid);
        assert_eq!(c.username, "alice");
        assert_eq!(c.role, "admin");
    }

    #[test]
    fn wrong_secret_rejects() {
        let a = JwtCodec::new("a".repeat(64).as_str(), 60);
        let b = JwtCodec::new("b".repeat(64).as_str(), 60);
        let tok = a.issue(Uuid::new_v4(), "x", "member").unwrap();
        assert!(b.verify(&tok).is_err());
    }
}
