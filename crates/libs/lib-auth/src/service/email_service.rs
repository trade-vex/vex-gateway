use lettre::{
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
    message::header::ContentType,
    transport::smtp::{SUBMISSIONS_PORT, authentication::Credentials},
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
const PASSWORD_RESET_ATTEMPT_PREFIX: &str = "otp:password_reset_attempts:";
const PASSWORD_RESET_TOKEN_PREFIX: &str = "password_reset:";
const AUTH_RATE_LIMIT_PREFIX: &str = "rate_limit:auth:";
const LOGIN_INFLIGHT_PREFIX: &str = "auth_rl:login_inflight:";
const LOGIN_INFLIGHT_TTL_SECONDS: u64 = 5;
const OTP_VERIFY_ATTEMPT_PREFIX: &str = "otp:verification_attempts:";
const OTP_VERIFY_MAX_ATTEMPTS: u64 = 5;
const PASSWORD_RESET_RATE_LIMIT_SCOPE: &str = "password_reset";
const PASSWORD_RESET_MAX_REQUESTS: u64 = 3;
const PASSWORD_RESET_WINDOW_SECONDS: u64 = 60 * 60;

#[derive(Clone)]
pub struct EmailService {
    smtp_transport: AsyncSmtpTransport<Tokio1Executor>,
    conn: ConnectionManager,
    from_email: String,
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
            tracing::info!("Configuring SMTP for production with TLS");
            // 465 is TLS from the first byte; 587 and others start plain and upgrade with STARTTLS.
            let builder = if email_config.smtp_port == SUBMISSIONS_PORT {
                AsyncSmtpTransport::<Tokio1Executor>::relay(&email_config.smtp_host)
            } else {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&email_config.smtp_host)
            };
            builder
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

    /// A six-digit code: the length the request DTOs accept (`length(min = 6, max = 6)`).
    fn generate_otp(&self) -> String {
        format!("{:06}", rand::thread_rng().gen_range(0..1_000_000))
    }

    /// Store a verification code for `email`.
    async fn store_otp(&self, email: &str, otp: &str) -> Result<(), AuthError> {
        self.store_code(
            OTP_KEY_PREFIX,
            OTP_VERIFY_ATTEMPT_PREFIX,
            email,
            otp,
            self.otp_expiry_minutes,
        )
        .await
    }

    /// Store a password-reset code for `email`.
    async fn store_password_reset_otp(&self, email: &str, otp: &str) -> Result<(), AuthError> {
        self.store_code(
            PASSWORD_RESET_OTP_PREFIX,
            PASSWORD_RESET_ATTEMPT_PREFIX,
            email,
            otp,
            self.password_reset_expiry_minutes,
        )
        .await
    }

    /// Store a code for its lifetime, replacing the last one; a new code starts with no failed
    /// attempts.
    async fn store_code(
        &self,
        prefix: &str,
        attempt_prefix: &str,
        email: &str,
        otp: &str,
        minutes: i64,
    ) -> Result<(), AuthError> {
        let expiry_seconds = Self::expiry_seconds(minutes)?;
        let mut conn = self.conn.clone();
        let _: () = redis::pipe()
            .atomic()
            .set_ex(code_key(prefix, email), otp, expiry_seconds)
            .del(code_key(attempt_prefix, email))
            .query_async(&mut conn)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store code: {}", e)))?;
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

    /// Check and consume the verification code for `email`.
    pub async fn verify_otp(&self, email: &str, submitted_otp: &str) -> Result<(), AuthError> {
        self.verify_code(
            OTP_KEY_PREFIX,
            OTP_VERIFY_ATTEMPT_PREFIX,
            email,
            submitted_otp,
        )
        .await
    }

    /// Check and consume the password-reset code for `email`.
    pub async fn verify_password_reset_otp(
        &self,
        email: &str,
        submitted_otp: &str,
    ) -> Result<(), AuthError> {
        self.verify_code(
            PASSWORD_RESET_OTP_PREFIX,
            PASSWORD_RESET_ATTEMPT_PREFIX,
            email,
            submitted_otp,
        )
        .await
    }

    /// One round trip: a matching code is deleted, so it works once; a wrong one counts an
    /// attempt, and at `OTP_VERIFY_MAX_ATTEMPTS` the code is deleted.
    async fn verify_code(
        &self,
        prefix: &str,
        attempt_prefix: &str,
        email: &str,
        submitted_otp: &str,
    ) -> Result<(), AuthError> {
        let mut conn = self.conn.clone();
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
        .key(code_key(prefix, email))
        .key(code_key(attempt_prefix, email))
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

    /// Generate a new password reset token
    pub fn generate_reset_token(&self) -> String {
        Uuid::new_v4().to_string()
    }

    /// Validate and consume password reset token
    /// Returns user_id if token is valid
    pub async fn validate_reset_token(&self, reset_token: &str) -> Result<Uuid, AuthError> {
        let mut conn = self.conn.clone();

        let token_key = format!("{}{}", PASSWORD_RESET_TOKEN_PREFIX, reset_token);

        // GETDEL: two requests with the same token cannot both redeem it.
        let user_id: Option<String> = conn
            .get_del(&token_key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to redeem reset token: {}", e)))?;
        let user_id = user_id.ok_or(AuthError::ResetTokenExpired)?;
        Uuid::parse_str(&user_id).map_err(|_| AuthError::InvalidResetToken)
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

/// The Redis key of a code (or its attempt count) for `email`, whatever case it was typed in: the
/// users table matches emails case-insensitively (CITEXT).
fn code_key(prefix: &str, email: &str) -> String {
    format!("{prefix}{}", email.trim().to_lowercase())
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
            email_verification_expiry_minutes: 15,
            password_reset_expiry_minutes: 15,
        })
        .await
    }

    #[tokio::test]
    async fn each_code_expires_after_its_own_lifetime_setting() {
        let service = email_service_with(&OtpConfig {
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
    async fn a_code_stored_for_one_spelling_of_the_email_verifies_for_another() {
        let service = email_service().await;
        let local = Uuid::new_v4().simple().to_string();
        let typed_at_signup = format!("{}@Example.COM", local.to_uppercase());
        let typed_later = format!(" {local}@example.com ");

        service.store_otp(&typed_at_signup, "123456").await.unwrap();
        service
            .store_password_reset_otp(&typed_at_signup, "654321")
            .await
            .unwrap();

        service.verify_otp(&typed_later, "123456").await.unwrap();
        service
            .verify_password_reset_otp(&typed_later, "654321")
            .await
            .unwrap();
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

    #[tokio::test]
    async fn password_reset_code_works_once_and_dies_at_the_attempt_cap() {
        let service = email_service().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        service
            .store_password_reset_otp(&email, "123456")
            .await
            .unwrap();

        for _ in 0..OTP_VERIFY_MAX_ATTEMPTS - 1 {
            assert!(matches!(
                service.verify_password_reset_otp(&email, "000000").await,
                Err(AuthError::InvalidOtp)
            ));
        }
        assert!(matches!(
            service.verify_password_reset_otp(&email, "000000").await,
            Err(AuthError::RateLimited { .. })
        ));
        assert!(matches!(
            service.verify_password_reset_otp(&email, "123456").await,
            Err(AuthError::OtpExpired)
        ));

        // A new code starts with a fresh attempt count and works once.
        service
            .store_password_reset_otp(&email, "654321")
            .await
            .unwrap();
        service
            .verify_password_reset_otp(&email, "654321")
            .await
            .unwrap();
        assert!(matches!(
            service.verify_password_reset_otp(&email, "654321").await,
            Err(AuthError::OtpExpired)
        ));
    }

    #[tokio::test]
    async fn concurrent_requests_redeem_a_reset_code_and_a_reset_token_once() {
        let service = email_service().await;
        let email = format!("{}@example.com", Uuid::new_v4());
        let token = service.generate_reset_token();
        service
            .store_password_reset_otp(&email, "123456")
            .await
            .unwrap();
        service
            .store_reset_token(&token, Uuid::new_v4())
            .await
            .unwrap();

        let tasks: Vec<_> = (0..10)
            .map(|_| {
                let (service, email, token) = (service.clone(), email.clone(), token.clone());
                tokio::spawn(async move {
                    (
                        service
                            .verify_password_reset_otp(&email, "123456")
                            .await
                            .is_ok(),
                        service.validate_reset_token(&token).await.is_ok(),
                    )
                })
            })
            .collect();
        let (mut codes, mut tokens) = (0, 0);
        for task in tasks {
            let (code, token) = task.await.unwrap();
            codes += usize::from(code);
            tokens += usize::from(token);
        }

        assert_eq!((codes, tokens), (1, 1));
    }

    #[tokio::test]
    async fn generated_codes_fit_the_requests_that_carry_them() {
        use crate::dto::auth_request_dto::{VerifyEmailRequest, VerifyResetOtpRequest};
        use validator::Validate;

        let service = email_service().await;
        for _ in 0..100 {
            let otp = service.generate_otp();
            assert!(otp.bytes().all(|b| b.is_ascii_digit()), "{otp}");
            let verify = VerifyEmailRequest {
                email: "user@example.com".to_string(),
                password: "correct-horse-battery-staple".to_string(),
                otp: otp.clone(),
            };
            let reset = VerifyResetOtpRequest {
                email: "user@example.com".to_string(),
                otp,
            };
            assert!(verify.validate().is_ok() && reset.validate().is_ok());
        }
    }

    #[tokio::test]
    async fn smtp_on_a_port_other_than_465_starts_plain_and_upgrades_with_starttls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // 127.0.0.2 is loopback but not a local-development host: the production transport.
        // macOS has no 127.0.0.2 without an interface alias; skip there.
        let Ok(server) = tokio::net::TcpListener::bind("127.0.0.2:0").await else {
            return;
        };
        let config = EmailConfig {
            smtp_host: "127.0.0.2".to_string(),
            smtp_port: server.local_addr().unwrap().port(),
            smtp_username: "test".to_string(),
            smtp_password: SecretString::new("test".to_string()),
            from_email: "test@example.com".to_string(),
        };
        let otp_config = OtpConfig {
            email_verification_expiry_minutes: 15,
            password_reset_expiry_minutes: 15,
        };
        let service = EmailService::new(&config, crate::test_redis().await, &otp_config).unwrap();
        let first_words = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            socket.write_all(b"220 test ESMTP\r\n").await.unwrap();
            let mut first = [0; 4];
            socket.read_exact(&mut first).await.unwrap();
            first
        });

        // The send fails once the server hangs up; only what the client said first matters.
        let _ = service
            .send_email("user@example.com", "Subject", "<p>html</p>", "text")
            .await;

        // A STARTTLS client answers the greeting in plain SMTP; an implicit-TLS client would have
        // opened with a TLS handshake record instead.
        assert_eq!(&first_words.await.unwrap(), b"EHLO");
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
