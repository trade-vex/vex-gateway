use lettre::{
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor, message::header::ContentType,
    transport::smtp::authentication::Credentials,
};
use lib_middleware::rate_limit::{self, Limit};
use rand::Rng;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use uuid::Uuid;

use crate::config::{EmailConfig, OtpConfig, code_expiry_seconds};
use crate::error::auth_error::AuthError;
use secrecy::ExposeSecret;

const OTP_KEY_PREFIX: &str = "otp:verification:";
const PASSWORD_RESET_OTP_PREFIX: &str = "otp:password_reset:";
const PASSWORD_RESET_TOKEN_PREFIX: &str = "password_reset:";
const AUTH_RATE_LIMIT_PREFIX: &str = "rate_limit:auth:";
const LOGIN_INFLIGHT_PREFIX: &str = "auth_rl:login_inflight:";
const LOGIN_INFLIGHT_TTL_SECONDS: u64 = 5;
const OTP_VERIFY_ATTEMPT_PREFIX: &str = "otp:verification_attempts:";
const OTP_VERIFY_MAX_ATTEMPTS: u64 = 5;
const PASSWORD_RESET_RATE_LIMIT_SCOPE: &str = "password_reset";
const PASSWORD_RESET_MAX_REQUESTS: u64 = 3;
const PASSWORD_RESET_WINDOW_SECONDS: u64 = 60 * 60;
const PASSWORD_RESET_OTP_RATE_LIMIT_SCOPE: &str = "password_reset_otp";
const PASSWORD_RESET_OTP_MAX_REQUESTS: u64 = 5;
const PASSWORD_RESET_OTP_WINDOW_SECONDS: u64 = 15 * 60;

#[derive(Clone)]
pub struct EmailService {
    smtp_transport: AsyncSmtpTransport<Tokio1Executor>,
    conn: ConnectionManager,
    from_email: String,
    otp_length: usize,
    otp_expiry_minutes: i64,
    password_reset_expiry_minutes: i64,
}

impl EmailService {
    pub fn new(
        email_config: &EmailConfig,
        conn: ConnectionManager,
        otp_config: &OtpConfig,
    ) -> Result<Self, AuthError> {
        tracing::info!("Initializing EmailService with SMTP configuration");
        tracing::info!("SMTP Host: {}", email_config.smtp_host);
        tracing::info!("SMTP Port: {}", email_config.smtp_port);
        tracing::info!("SMTP Username: {}", email_config.smtp_username);
        tracing::info!("From Email: {}", email_config.from_email);
        tracing::info!("OTP Length: {}", otp_config.otp_length);
        tracing::info!(
            "OTP Expiry Minutes: {} (email verification), {} (password reset)",
            otp_config.email_verification_expiry_minutes,
            otp_config.password_reset_expiry_minutes
        );

        let creds = Credentials::new(
            email_config.smtp_username.clone(),
            email_config.smtp_password.expose_secret().clone(),
        );

        tracing::debug!(
            "Creating SMTP transport for host: {}",
            email_config.smtp_host
        );

        // Configure SMTP transport based on host
        let smtp_transport = if email_config.smtp_host == "localhost"
            || email_config.smtp_host == "127.0.0.1"
            || email_config.smtp_host == "host.docker.internal"
        {
            // for local development, use plain text without TLS
            tracing::info!("Configuring SMTP for local development (no TLS)");
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&email_config.smtp_host)
                .port(email_config.smtp_port)
                .build()
        } else {
            // for production SMTP servers, use relay with TLS
            tracing::info!("Configuring SMTP for production with TLS");
            AsyncSmtpTransport::<Tokio1Executor>::relay(&email_config.smtp_host)
                .map_err(|e| {
                    tracing::error!(
                        "SMTP relay error for host {}: {}",
                        email_config.smtp_host,
                        e
                    );
                    AuthError::InternalError(format!("SMTP relay error: {}", e))
                })?
                .credentials(creds)
                .port(email_config.smtp_port)
                .build()
        };

        tracing::info!("EmailService initialized successfully");
        Ok(Self {
            smtp_transport,
            conn,
            from_email: email_config.from_email.clone(),
            otp_length: otp_config.otp_length,
            otp_expiry_minutes: otp_config.email_verification_expiry_minutes,
            password_reset_expiry_minutes: otp_config.password_reset_expiry_minutes,
        })
    }

    /// A code's Redis TTL: its lifetime setting in seconds. lib-config rejects a lifetime that has
    /// none at load, so this fails only for a service built from a configuration it did not load.
    fn expiry_seconds(minutes: i64) -> Result<u64, AuthError> {
        code_expiry_seconds(minutes).ok_or_else(|| {
            tracing::error!("Invalid OTP expiry minutes: {}", minutes);
            AuthError::Validation("OTP expiry minutes must be positive".to_string())
        })
    }

    /// Generate a numeric OTP
    fn generate_otp(&self) -> String {
        let mut rng = rand::thread_rng();
        let mut otp = String::with_capacity(self.otp_length);

        for _ in 0..self.otp_length {
            let digit = rng.gen_range(0..10);
            otp.push_str(&digit.to_string());
        }

        otp
    }

    /// Store OTP in Redis
    async fn store_otp(&self, email: &str, otp: &str) -> Result<(), AuthError> {
        tracing::debug!("Storing OTP in Redis for email: {}", email);

        let expiry_seconds_u64 = Self::expiry_seconds(self.otp_expiry_minutes)?;

        tracing::debug!("Getting Redis connection from pool");
        let mut conn = self.conn.clone();

        let key = format!("{}{}", OTP_KEY_PREFIX, email);

        tracing::debug!("Redis key: {}", key);
        tracing::debug!(
            "OTP expiry: {} minutes ({} seconds)",
            self.otp_expiry_minutes,
            expiry_seconds_u64
        );

        tracing::debug!(
            "Storing OTP in Redis with key: {} and expiry: {} seconds",
            key,
            expiry_seconds_u64
        );
        let attempt_key = format!("{}{}", OTP_VERIFY_ATTEMPT_PREFIX, email);
        let _: () = redis::pipe()
            .atomic()
            .set_ex(&key, otp, expiry_seconds_u64)
            .del(attempt_key)
            .query_async(&mut conn)
            .await
            .map_err(|e| {
                tracing::error!("Failed to store OTP in Redis: {}", e);
                AuthError::Redis(format!("Failed to store OTP: {}", e))
            })?;

        tracing::debug!("OTP stored successfully in Redis");
        Ok(())
    }

    /// Store a new verification code for `email` and send it there.
    pub async fn send_verification_email(&self, email: &str) -> Result<(), AuthError> {
        let otp = self.generate_otp();
        self.store_otp(email, &otp).await?;
        self.send_code(
            email,
            "Verify Your Email",
            "Email Verification",
            "Thank you for signing up! Your verification code is:",
            &otp,
            self.otp_expiry_minutes,
        )
        .await
    }

    /// Send `otp` to `recipient_email` in the one code email (`code_email`).
    async fn send_code(
        &self,
        recipient_email: &str,
        subject: &str,
        heading: &str,
        intro: &str,
        otp: &str,
        minutes: i64,
    ) -> Result<(), AuthError> {
        let (text, html) = code_email(heading, intro, otp, minutes);
        self.send_email(recipient_email, subject, &html, &text)
            .await
    }

    /// Send an email to the bare address: nothing the user typed goes into the mail.
    async fn send_email(
        &self,
        recipient_email: &str,
        subject: &str,
        html_content: &str,
        text_content: &str,
    ) -> Result<(), AuthError> {
        tracing::info!("Building email message");
        tracing::debug!("From: {}", self.from_email);
        tracing::debug!("Subject: {}", subject);

        let email = lettre::Message::builder()
            .from(self.from_email.parse().map_err(|e| {
                tracing::error!("Invalid from email '{}': {}", self.from_email, e);
                AuthError::InternalError(format!("Invalid from email: {}", e))
            })?)
            .to(recipient_email.parse().map_err(|e| {
                tracing::error!("Invalid recipient email: {}", e);
                AuthError::InternalError(format!("Invalid recipient email: {}", e))
            })?)
            .subject(subject)
            .multipart(
                lettre::message::MultiPart::alternative()
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(text_content.to_string()),
                    )
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html_content.to_string()),
                    ),
            )
            .map_err(|e| {
                tracing::error!("Failed to build email message: {}", e);
                AuthError::InternalError(format!("Failed to build email: {}", e))
            })?;

        tracing::info!("Email message built successfully, attempting to send via SMTP");
        tracing::debug!("SMTP transport ready, calling send() method");

        let start_time = std::time::Instant::now();

        // Add timeout to prevent hanging
        let send_future = self.smtp_transport.send(email);
        let timeout_duration = std::time::Duration::from_secs(30); // 30 second timeout

        tracing::debug!(
            "Starting SMTP send with {} second timeout",
            timeout_duration.as_secs()
        );

        let result = tokio::time::timeout(timeout_duration, send_future)
            .await
            .map_err(|_| {
                let elapsed = start_time.elapsed();
                tracing::error!(
                    "SMTP send timed out after {:?} (timeout: {:?})",
                    elapsed,
                    timeout_duration
                );
                AuthError::InternalError(format!(
                    "SMTP send timed out after {:?}",
                    timeout_duration
                ))
            })?
            .map_err(|e| {
                let elapsed = start_time.elapsed();
                tracing::error!("SMTP send failed after {:?}: {}", elapsed, e);
                AuthError::InternalError(format!("Failed to send email: {}", e))
            });

        match &result {
            Ok(response) => {
                let elapsed = start_time.elapsed();
                tracing::info!(
                    "Email sent successfully via SMTP in {:?}, response: {:?}",
                    elapsed,
                    response
                );
            }
            Err(e) => {
                let elapsed = start_time.elapsed();
                tracing::error!("SMTP send error after {:?}: {}", elapsed, e);
            }
        }

        result.map(|_| ())
    }

    /// Verify OTP
    pub async fn verify_otp(&self, email: &str, submitted_otp: &str) -> Result<(), AuthError> {
        let mut conn = self.conn.clone();

        let key = format!("{}{}", OTP_KEY_PREFIX, email);
        let attempt_key = format!("{}{}", OTP_VERIFY_ATTEMPT_PREFIX, email);
        let result: (u8, u64, i64) = redis::Script::new(
            r#"
local otp = redis.call('GET', KEYS[1])
if not otp then
    return {0, 0, 0}
end

if otp == ARGV[1] then
    redis.call('DEL', KEYS[1], KEYS[2])
    return {1, 0, 0}
end

local ttl = redis.call('TTL', KEYS[1])
local attempts = redis.call('INCR', KEYS[2])
if attempts == 1 and ttl > 0 then
    redis.call('EXPIRE', KEYS[2], ttl)
end

if attempts >= tonumber(ARGV[2]) then
    redis.call('DEL', KEYS[1])
    return {3, attempts, ttl}
end

return {2, attempts, ttl}
"#,
        )
        .key(&key)
        .key(&attempt_key)
        .arg(submitted_otp)
        .arg(OTP_VERIFY_MAX_ATTEMPTS)
        .invoke_async(&mut conn)
        .await
        .map_err(|e| AuthError::Redis(format!("Failed to verify OTP: {}", e)))?;

        match result.0 {
            0 => Err(AuthError::OtpExpired),
            1 => Ok(()),
            2 => Err(AuthError::InvalidOtp),
            3 => Err(AuthError::RateLimited {
                retry_after_seconds: result.2.max(1) as u64,
            }),
            _ => Err(AuthError::InternalError(
                "Unexpected OTP verification result".to_string(),
            )),
        }
    }

    /// Enforce a fixed-window Redis-backed rate limit (`lib_middleware::rate_limit`).
    /// Redis failures are returned to the handler so authentication fails closed.
    pub async fn enforce_rate_limit(
        &self,
        scope: &str,
        identity: &str,
        max_requests: u64,
        window_seconds: u64,
    ) -> Result<(), AuthError> {
        let key = format!(
            "{}{}:{}",
            AUTH_RATE_LIMIT_PREFIX,
            scope,
            identity.trim().to_ascii_lowercase()
        );
        let limit = Limit {
            max_requests,
            window_seconds,
        };

        Self::rate_limit_result(rate_limit::hit(&self.conn, &key, limit).await)
    }

    /// Reject a login before password verification when its failure budget is exhausted.
    pub async fn check_login_failure_rate_limit(
        &self,
        identity: &str,
        max_failures: u64,
        window_seconds: u64,
    ) -> Result<(), AuthError> {
        let key = format!(
            "{}login:{}",
            AUTH_RATE_LIMIT_PREFIX,
            identity.trim().to_ascii_lowercase()
        );
        let limit = Limit {
            max_requests: max_failures,
            window_seconds,
        };

        Self::rate_limit_result(rate_limit::check(&self.conn, &key, limit).await)
    }

    /// Reserve password verification for one request per login principal.
    pub async fn acquire_login_inflight_lock(&self, identity: &str) -> Result<String, AuthError> {
        let mut conn = self.conn.clone();
        let key = format!(
            "{}{}",
            LOGIN_INFLIGHT_PREFIX,
            identity.trim().to_ascii_lowercase()
        );
        let token = Uuid::new_v4().to_string();
        let acquired: Option<String> = redis::cmd("SET")
            .arg(key)
            .arg(&token)
            .arg("NX")
            .arg("EX")
            .arg(LOGIN_INFLIGHT_TTL_SECONDS)
            .query_async(&mut conn)
            .await
            .map_err(|e| {
                AuthError::Redis(format!("Failed to acquire login in-flight lock: {}", e))
            })?;

        if acquired.is_none() {
            return Err(AuthError::RateLimited {
                retry_after_seconds: LOGIN_INFLIGHT_TTL_SECONDS,
            });
        }

        Ok(token)
    }

    /// Release a login reservation only if it is still owned by this request.
    pub async fn release_login_inflight_lock(
        &self,
        identity: &str,
        token: &str,
    ) -> Result<(), AuthError> {
        let mut conn = self.conn.clone();
        let key = format!(
            "{}{}",
            LOGIN_INFLIGHT_PREFIX,
            identity.trim().to_ascii_lowercase()
        );
        let _: u64 = redis::Script::new(
            r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
    return redis.call('DEL', KEYS[1])
end
return 0
"#,
        )
        .key(key)
        .arg(token)
        .invoke_async(&mut conn)
        .await
        .map_err(|e| AuthError::Redis(format!("Failed to release login in-flight lock: {}", e)))?;

        Ok(())
    }

    /// Atomically record a failed login or clear prior failures after authentication succeeds.
    pub async fn record_login_authentication(
        &self,
        identity: &str,
        authenticated: bool,
        window_seconds: u64,
    ) -> Result<(), AuthError> {
        let mut conn = self.conn.clone();
        let key = format!(
            "{}login:{}",
            AUTH_RATE_LIMIT_PREFIX,
            identity.trim().to_ascii_lowercase()
        );
        let result: redis::RedisResult<u64> = redis::Script::new(
            r#"
if ARGV[2] == '1' then
    return redis.call('DEL', KEYS[1])
end
local count = redis.call('INCR', KEYS[1])
if count == 1 then
    redis.call('EXPIRE', KEYS[1], ARGV[1])
end
return count
"#,
        )
        .key(key)
        .arg(window_seconds)
        .arg(u8::from(authenticated))
        .invoke_async(&mut conn)
        .await;

        result
            .map(|_| ())
            .map_err(|e| AuthError::Redis(format!("Failed to record login authentication: {}", e)))
    }

    fn rate_limit_result(result: redis::RedisResult<Option<u64>>) -> Result<(), AuthError> {
        match result
            .map_err(|e| AuthError::Redis(format!("Failed to enforce rate limit: {}", e)))?
        {
            None => Ok(()),
            Some(retry_after_seconds) => Err(AuthError::RateLimited {
                retry_after_seconds,
            }),
        }
    }

    /// Enforce the existing password-reset request limit.
    pub async fn enforce_password_reset_rate_limit(&self, email: &str) -> Result<(), AuthError> {
        self.enforce_rate_limit(
            PASSWORD_RESET_RATE_LIMIT_SCOPE,
            email,
            PASSWORD_RESET_MAX_REQUESTS,
            PASSWORD_RESET_WINDOW_SECONDS,
        )
        .await
    }

    /// Enforce the existing password-reset OTP verification limit.
    async fn enforce_password_reset_otp_rate_limit(&self, email: &str) -> Result<(), AuthError> {
        self.enforce_rate_limit(
            PASSWORD_RESET_OTP_RATE_LIMIT_SCOPE,
            email,
            PASSWORD_RESET_OTP_MAX_REQUESTS,
            PASSWORD_RESET_OTP_WINDOW_SECONDS,
        )
        .await
    }

    /// Store password reset OTP in Redis
    async fn store_password_reset_otp(&self, email: &str, otp: &str) -> Result<(), AuthError> {
        let expiry_seconds_u64 = Self::expiry_seconds(self.password_reset_expiry_minutes)?;
        let mut conn = self.conn.clone();

        let key = format!("{}{}", PASSWORD_RESET_OTP_PREFIX, email);

        let _: () = conn
            .set_ex(&key, otp, expiry_seconds_u64)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store password reset OTP: {}", e)))?;

        Ok(())
    }

    /// Store a new password-reset code for `email` and send it there.
    pub async fn send_password_reset_otp(&self, email: &str) -> Result<(), AuthError> {
        let otp = self.generate_otp();
        self.store_password_reset_otp(email, &otp).await?;
        self.send_code(
            email,
            "Reset Your Password",
            "Password Reset",
            "You requested to reset your password. Your verification code is:",
            &otp,
            self.password_reset_expiry_minutes,
        )
        .await
    }

    /// Verify password reset OTP
    /// Returns Ok(()) if OTP is valid, deletes OTP (one-time use)
    pub async fn verify_password_reset_otp(
        &self,
        email: &str,
        submitted_otp: &str,
    ) -> Result<(), AuthError> {
        self.enforce_password_reset_otp_rate_limit(email).await?;

        let mut conn = self.conn.clone();

        let key = format!("{}{}", PASSWORD_RESET_OTP_PREFIX, email);

        let stored_otp: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get password reset OTP: {}", e)))?;

        match stored_otp {
            Some(otp) if otp == submitted_otp => {
                // Delete OTP after successful verification (one-time use)
                let _: () = conn
                    .del(&key)
                    .await
                    .map_err(|e| AuthError::Redis(format!("Failed to delete OTP: {}", e)))?;

                // Clean up OTP verification rate limit since OTP is no longer valid
                let rate_limit_key = format!(
                    "{}{}:{}",
                    AUTH_RATE_LIMIT_PREFIX,
                    PASSWORD_RESET_OTP_RATE_LIMIT_SCOPE,
                    email.trim().to_ascii_lowercase()
                );
                let _: () = conn
                    .del(&rate_limit_key)
                    .await
                    .map_err(|e| {
                        tracing::warn!("Failed to delete OTP verify rate limit: {}", e);
                        // Don't fail the request if cleanup fails
                    })
                    .unwrap_or(());

                Ok(())
            }
            Some(_) => Err(AuthError::InvalidOtp),
            None => Err(AuthError::OtpExpired),
        }
    }

    /// Generate a new password reset token
    pub fn generate_reset_token(&self) -> String {
        Uuid::new_v4().to_string()
    }

    /// Validate and consume password reset token
    /// Returns user_id if token is valid
    pub async fn validate_reset_token(&self, reset_token: &str) -> Result<Uuid, AuthError> {
        let mut conn = self.conn.clone();

        let token_key = format!("{}{}", PASSWORD_RESET_TOKEN_PREFIX, reset_token);

        let user_id_str: Option<String> = conn
            .get(&token_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get reset token: {}", e)))?;

        match user_id_str {
            Some(user_id_str) => {
                // Delete token after validation (one-time use)
                let _: () = conn.del(&token_key).await.map_err(|e| {
                    AuthError::Redis(format!("Failed to delete reset token: {}", e))
                })?;

                // Parse user_id
                if user_id_str == "pending" {
                    return Err(AuthError::InvalidResetToken);
                }

                let user_id =
                    Uuid::parse_str(&user_id_str).map_err(|_| AuthError::InvalidResetToken)?;

                Ok(user_id)
            }
            None => Err(AuthError::ResetTokenExpired),
        }
    }

    /// Store reset token with user_id
    pub async fn store_reset_token(
        &self,
        reset_token: &str,
        user_id: Uuid,
    ) -> Result<(), AuthError> {
        let mut conn = self.conn.clone();

        let token_key = format!("{}{}", PASSWORD_RESET_TOKEN_PREFIX, reset_token);

        let _: () = conn
            .set_ex(&token_key, user_id.to_string(), 1800) // 30 minutes
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store reset token: {}", e)))?;

        Ok(())
    }
}

/// The text and HTML bodies of a code email, the same for every code the gateway sends.
fn code_email(heading: &str, intro: &str, otp: &str, minutes: i64) -> (String, String) {
    let text = format!(
        "Hello,\n\n{intro}\n\n{otp}\n\nThis code will expire in {minutes} minutes.\n\n\
         If you didn't request this code, please ignore this email.\n\nBest regards,\nThe Team"
    );
    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"></head>
<body style="font-family: Arial, sans-serif; line-height: 1.6; color: #333; max-width: 600px; margin: 0 auto; padding: 20px;">
    <h2 style="text-align: center;">{heading}</h2>
    <p>Hello,</p>
    <p>{intro}</p>
    <div style="font-size: 32px; font-weight: bold; letter-spacing: 8px; text-align: center; padding: 20px; margin: 20px 0; background-color: #f0f0f0; border-radius: 8px;">{otp}</div>
    <p>This code will expire in <strong>{minutes} minutes</strong>.</p>
    <p>If you didn't request this code, please ignore this email.</p>
    <p>Best regards,<br>The Team</p>
</body>
</html>"#
    );
    (text, html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EmailConfig;
    use secrecy::SecretString;

    /// A service on the tests' Redis ([`crate::test_redis`]) with these code settings.
    async fn email_service_with(otp_config: &OtpConfig) -> EmailService {
        let conn = crate::test_redis().await;
        let config = EmailConfig {
            smtp_host: "127.0.0.1".to_string(),
            smtp_port: 2525,
            smtp_username: "test".to_string(),
            smtp_password: SecretString::new("test".to_string()),
            from_email: "test@example.com".to_string(),
        };

        EmailService::new(&config, conn, otp_config).unwrap()
    }

    async fn email_service() -> EmailService {
        email_service_with(&OtpConfig {
            otp_length: 6,
            email_verification_expiry_minutes: 15,
            password_reset_expiry_minutes: 15,
        })
        .await
    }

    #[tokio::test]
    async fn each_code_expires_after_its_own_lifetime_setting() {
        let service = email_service_with(&OtpConfig {
            otp_length: 6,
            email_verification_expiry_minutes: 20,
            password_reset_expiry_minutes: 7,
        })
        .await;
        let email = format!("{}@example.com", Uuid::new_v4());
        service.store_otp(&email, "123456").await.unwrap();
        service
            .store_password_reset_otp(&email, "654321")
            .await
            .unwrap();

        let mut conn = service.conn.clone();
        let verification_ttl: i64 = conn.ttl(format!("{OTP_KEY_PREFIX}{email}")).await.unwrap();
        let reset_ttl: i64 = conn
            .ttl(format!("{PASSWORD_RESET_OTP_PREFIX}{email}"))
            .await
            .unwrap();
        assert!(
            (20 * 60 - 5..=20 * 60).contains(&verification_ttl),
            "{verification_ttl}"
        );
        assert!((7 * 60 - 5..=7 * 60).contains(&reset_ttl), "{reset_ttl}");
    }

    #[tokio::test]
    async fn rate_limit_triggers_after_boundary() {
        let service = email_service().await;
        let identity = Uuid::new_v4().to_string();

        for _ in 0..3 {
            service
                .enforce_rate_limit("test_boundary", &identity, 3, 60)
                .await
                .unwrap();
        }

        let result = service
            .enforce_rate_limit("test_boundary", &identity, 3, 60)
            .await;
        assert!(matches!(
            result,
            Err(AuthError::RateLimited {
                retry_after_seconds: 1..=60
            })
        ));
    }

    #[tokio::test]
    async fn verification_otp_is_invalidated_at_attempt_cap() {
        let service = email_service().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        service.store_otp(&email, "123456").await.unwrap();

        for _ in 0..OTP_VERIFY_MAX_ATTEMPTS - 1 {
            assert!(matches!(
                service.verify_otp(&email, "000000").await,
                Err(AuthError::InvalidOtp)
            ));
        }

        assert!(matches!(
            service.verify_otp(&email, "000000").await,
            Err(AuthError::RateLimited { .. })
        ));
        assert!(matches!(
            service.verify_otp(&email, "123456").await,
            Err(AuthError::OtpExpired)
        ));

        service.store_otp(&email, "654321").await.unwrap();
        service.verify_otp(&email, "654321").await.unwrap();
    }

    #[test]
    fn code_email_carries_the_code_and_its_lifetime() {
        let (text, html) = code_email("Heading", "Your code is:", "123456", 15);

        for body in [&text, &html] {
            assert!(body.contains("Your code is:"), "{body}");
            assert!(body.contains("123456"), "{body}");
            assert!(body.contains("15 minutes"), "{body}");
        }
        assert!(text.starts_with("Hello,\n"), "{text}");
        assert!(html.contains("<h2 style=\"text-align: center;\">Heading</h2>"));
    }

    #[test]
    fn redis_failure_rejects_rate_limited_operation() {
        let redis_error = redis::RedisError::from((redis::ErrorKind::Io, "Redis unavailable"));
        let result = EmailService::rate_limit_result(Err(redis_error));

        assert!(matches!(result, Err(AuthError::Redis(_))));
    }
}
