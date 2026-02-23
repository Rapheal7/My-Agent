//! JWT authentication
//!
//! Provides JWT-based authentication for API endpoints.
//! Supports access tokens and refresh tokens with configurable expiration.

use anyhow::{Result, Context, bail};
use sha2::{Sha256, Digest};

use chrono::{DateTime, Utc, Duration};
use jsonwebtoken::{encode, decode, Header, Algorithm, Validation, DecodingKey, EncodingKey};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use axum::{
    extract::{Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::Response,
};

/// JWT claims structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID)
    pub sub: String,
    /// Issued at
    pub iat: i64,
    /// Expiration time
    pub exp: i64,
    /// Token type (access or refresh)
    pub token_type: TokenType,
    /// User permissions/roles
    pub permissions: Vec<String>,
    /// Session ID for revocation
    pub jti: String,
}

/// Token type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenType {
    Access,
    Refresh,
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// JWT secret key (should be 256-bit for HS256)
    pub jwt_secret: String,
    /// Access token expiration (minutes)
    pub access_token_expiry_minutes: i64,
    /// Refresh token expiration (days)
    pub refresh_token_expiry_days: i64,
    /// Maximum failed login attempts before lockout
    pub max_login_attempts: u32,
    /// Lockout duration (minutes)
    pub lockout_duration_minutes: i64,
    /// Require HTTPS for authentication
    pub require_https: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: generate_jwt_secret(),
            access_token_expiry_minutes: 60,
            refresh_token_expiry_days: 7,
            max_login_attempts: 5,
            lockout_duration_minutes: 30,
            require_https: true,
        }
    }
}

/// User credentials (in production, use hashed passwords)
#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub permissions: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// Authentication state
pub struct AuthState {
    config: AuthConfig,
    /// Revoked token IDs (for logout)
    revoked_tokens: RwLock<HashMap<String, DateTime<Utc>>>,
    /// Failed login attempts
    login_attempts: RwLock<HashMap<String, (u32, DateTime<Utc>)>>,
    /// Active sessions
    sessions: RwLock<HashMap<String, SessionInfo>>,
}

/// Session information
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

impl AuthState {
    /// Create new auth state with config
    pub fn new(config: AuthConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            revoked_tokens: RwLock::new(HashMap::new()),
            login_attempts: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
        })
    }

    /// Generate access token for user
    pub fn generate_access_token(&self, user_id: &str, permissions: &[String]) -> Result<String> {
        let now = Utc::now();
        let expiry = now + Duration::minutes(self.config.access_token_expiry_minutes);
        let jti = uuid::Uuid::new_v4().to_string();

        let claims = Claims {
            sub: user_id.to_string(),
            iat: now.timestamp(),
            exp: expiry.timestamp(),
            token_type: TokenType::Access,
            permissions: permissions.to_vec(),
            jti: jti.clone(),
        };

        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(self.config.jwt_secret.as_bytes()),
        ).context("Failed to encode JWT")?;

        // Track session
        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(jti, SessionInfo {
            user_id: user_id.to_string(),
            created_at: now,
            last_active: now,
            ip_address: None,
            user_agent: None,
        });

        Ok(token)
    }

    /// Generate refresh token
    pub fn generate_refresh_token(&self, user_id: &str) -> Result<String> {
        let now = Utc::now();
        let expiry = now + Duration::days(self.config.refresh_token_expiry_days);
        let jti = uuid::Uuid::new_v4().to_string();

        let claims = Claims {
            sub: user_id.to_string(),
            iat: now.timestamp(),
            exp: expiry.timestamp(),
            token_type: TokenType::Refresh,
            permissions: vec![],
            jti,
        };

        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(self.config.jwt_secret.as_bytes()),
        ).context("Failed to encode refresh token")
    }

    /// Validate and decode token
    pub fn validate_token(&self, token: &str) -> Result<Claims> {
        // Check if token is revoked
        let jti = self.extract_jti(token)?;
        if self.is_token_revoked(&jti) {
            bail!("Token has been revoked");
        }

        let validation = Validation::new(Algorithm::HS256);
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.config.jwt_secret.as_bytes()),
            &validation,
        ).context("Invalid token")?;

        // Update last active
        if let Ok(mut sessions) = self.sessions.write() {
            if let Some(session) = sessions.get_mut(&token_data.claims.jti) {
                session.last_active = Utc::now();
            }
        }

        Ok(token_data.claims)
    }

    /// Extract JTI from token without full validation
    pub fn extract_jti(&self, token: &str) -> Result<String> {
        // Decode without validation to get JTI
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = false;
        validation.validate_nbf = false;

        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.config.jwt_secret.as_bytes()),
            &validation,
        ).context("Failed to decode token")?;

        Ok(token_data.claims.jti)
    }

    /// Revoke a token (logout)
    pub fn revoke_token(&self, jti: &str) -> Result<()> {
        let mut revoked = self.revoked_tokens.write().unwrap();
        revoked.insert(jti.to_string(), Utc::now());

        // Also remove from active sessions
        let mut sessions = self.sessions.write().unwrap();
        sessions.remove(jti);

        Ok(())
    }

    /// Check if token is revoked
    fn is_token_revoked(&self, jti: &str) -> bool {
        let revoked = self.revoked_tokens.read().unwrap();
        revoked.contains_key(jti)
    }

    /// Record failed login attempt
    pub fn record_failed_login(&self, identifier: &str) -> Result<()> {
        let mut attempts = self.login_attempts.write().unwrap();
        let entry = attempts.entry(identifier.to_string()).or_insert((0, Utc::now()));
        entry.0 += 1;
        entry.1 = Utc::now();
        Ok(())
    }

    /// Check if account is locked
    pub fn is_locked(&self, identifier: &str) -> Option<Duration> {
        let attempts = self.login_attempts.read().unwrap();
        if let Some((count, last_attempt)) = attempts.get(identifier) {
            if *count >= self.config.max_login_attempts {
                let lockout_end = *last_attempt + Duration::minutes(self.config.lockout_duration_minutes);
                let now = Utc::now();
                if now < lockout_end {
                    return Some(lockout_end - now);
                }
            }
        }
        None
    }

    /// Clear login attempts (on successful login)
    pub fn clear_login_attempts(&self, identifier: &str) {
        let mut attempts = self.login_attempts.write().unwrap();
        attempts.remove(identifier);
    }

    /// Get active sessions for user
    pub fn get_user_sessions(&self, user_id: &str) -> Vec<(String, SessionInfo)> {
        let sessions = self.sessions.read().unwrap();
        sessions
            .iter()
            .filter(|(_, info)| info.user_id == user_id)
            .map(|(jti, info)| (jti.clone(), info.clone()))
            .collect()
    }

    /// Clean up expired sessions and revoked tokens
    pub fn cleanup(&self) -> Result<()> {
        let now = Utc::now();

        // Clean up expired sessions
        {
            let mut sessions = self.sessions.write().unwrap();
            let expired_jtis: Vec<String> = sessions
                .iter()
                .filter(|(_, info)| {
                    let session_expiry = info.created_at + Duration::days(self.config.refresh_token_expiry_days);
                    now > session_expiry
                })
                .map(|(jti, _)| jti.clone())
                .collect();
            for jti in expired_jtis {
                sessions.remove(&jti);
            }
        }

        // Clean up old revoked tokens (keep for 7 days)
        {
            let mut revoked = self.revoked_tokens.write().unwrap();
            let old_jtis: Vec<String> = revoked
                .iter()
                .filter(|(_, revoked_at)| now - **revoked_at > Duration::days(7))
                .map(|(jti, _)| jti.clone())
                .collect();
            for jti in old_jtis {
                revoked.remove(&jti);
            }
        }

        Ok(())
    }
}

/// Generate a secure JWT secret
pub fn generate_jwt_secret() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes)
}

/// Hash a password (using argon2 in production, simple hash for demo)
pub fn hash_password(password: &str) -> Result<String> {
    // In production, use argon2 or bcrypt
    // For now, use a simple hash with salt
    use rand::Rng;
    let mut rng = rand::rng();
    let salt: [u8; 16] = rng.random();
    let salt_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &salt);

    let combined = format!("{}{}", password, salt_b64);
    let hash = sha2::Sha256::digest(combined.as_bytes());
    let hash_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &hash);

    Ok(format!("{}${}", salt_b64, hash_b64))
}

/// Verify a password hash
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parts: Vec<&str> = hash.split('$').collect();
    if parts.len() != 2 {
        bail!("Invalid hash format");
    }

    let salt_b64 = parts[0];
    let combined = format!("{}{}", password, salt_b64);
    let computed_hash = sha2::Sha256::digest(combined.as_bytes());
    let computed_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &computed_hash);

    Ok(computed_b64 == parts[1])
}

/// Axum middleware for JWT authentication
pub async fn auth_middleware(
    State(state): State<Arc<AuthState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract token from Authorization header
    let auth_header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "));

    let token = match auth_header {
        Some(token) => token,
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    // Validate token
    let claims = match state.validate_token(token) {
        Ok(claims) => claims,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };

    // Check if it's an access token
    if claims.token_type != TokenType::Access {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Add claims to request extensions for handlers
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

/// Extract claims from request extensions
pub fn extract_claims(request: &Request) -> Option<&Claims> {
    request.extensions().get::<Claims>()
}

/// Login request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
}

/// Refresh token request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// Logout request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogoutRequest {
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jwt_generation_and_validation() {
        let config = AuthConfig::default();
        let state = AuthState::new(config);

        let token = state.generate_access_token("user123", &["read".to_string(), "write".to_string()]).unwrap();
        let claims = state.validate_token(&token).unwrap();

        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.permissions, vec!["read", "write"]);
        assert_eq!(claims.token_type, TokenType::Access);
    }

    #[test]
    fn test_token_revocation() {
        let config = AuthConfig::default();
        let state = AuthState::new(config);

        let token = state.generate_access_token("user123", &[]).unwrap();
        let claims = state.validate_token(&token).unwrap();

        // Revoke the token
        state.revoke_token(&claims.jti).unwrap();

        // Should fail validation now
        assert!(state.validate_token(&token).is_err());
    }

    #[test]
    fn test_refresh_token() {
        let config = AuthConfig::default();
        let state = AuthState::new(config);

        let token = state.generate_refresh_token("user123").unwrap();
        let claims = state.validate_token(&token).unwrap();

        assert_eq!(claims.token_type, TokenType::Refresh);
    }

    #[test]
    fn test_password_hashing() {
        let password = "my_secure_password";
        let hash = hash_password(password).unwrap();

        // Should verify correctly
        assert!(verify_password(password, &hash).unwrap());

        // Wrong password should fail
        assert!(!verify_password("wrong_password", &hash).unwrap());
    }
}
