use lettre::{
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor, message::header::ContentType,
    transport::smtp::authentication::Credentials,
};
use rand::Rng;
use redis::AsyncCommands;
use uuid::Uuid;

use crate::config::EmailConfig;
use crate::error::auth_error::AuthError;
use crate::redis_pool::RedisPool;
use secrecy::ExposeSecret;

const OTP_KEY_PREFIX: &str = "otp:verification:";
const PASSWORD_RESET_OTP_PREFIX: &str = "otp:password_reset:";
const PASSWORD_RESET_TOKEN_PREFIX: &str = "password_reset:";
const RATE_LIMIT_PASSWORD_RESET_PREFIX: &str = "rate_limit:password_reset:";
const RATE_LIMIT_OTP_VERIFY_PREFIX: &str = "rate_limit:otp_verify:";

#[derive(Clone)]
pub struct EmailService {
    smtp_transport: AsyncSmtpTransport<Tokio1Executor>,
    pool: RedisPool,
    from_email: String,
    otp_length: usize,
    otp_expiry_minutes: i64,
}

impl EmailService {
    pub fn new(
        email_config: &EmailConfig,
        pool: RedisPool,
        otp_length: usize,
        otp_expiry_minutes: i64,
    ) -> Result<Self, AuthError> {
        tracing::info!("Initializing EmailService with SMTP configuration");
        tracing::info!("SMTP Host: {}", email_config.smtp_host);
        tracing::info!("SMTP Port: {}", email_config.smtp_port);
        tracing::info!("SMTP Username: {}", email_config.smtp_username);
        tracing::info!("From Email: {}", email_config.from_email);
        tracing::info!("OTP Length: {}", otp_length);
        tracing::info!("OTP Expiry Minutes: {}", otp_expiry_minutes);

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
            pool,
            from_email: email_config.from_email.clone(),
            otp_length,
            otp_expiry_minutes,
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

        // Validate otp_expiry_minutes is positive
        if self.otp_expiry_minutes <= 0 {
            tracing::error!("Invalid OTP expiry minutes: {}", self.otp_expiry_minutes);
            return Err(AuthError::Validation(
                "OTP expiry minutes must be positive".to_string(),
            ));
        }

        tracing::debug!("Getting Redis connection from pool");
        let mut conn = self.pool.get();

        let key = format!("{}{}", OTP_KEY_PREFIX, email);
        let expiry_seconds = self.otp_expiry_minutes * 60;

        tracing::debug!("Redis key: {}", key);
        tracing::debug!(
            "OTP expiry: {} minutes ({} seconds)",
            self.otp_expiry_minutes,
            expiry_seconds
        );

        // Convert to u64 using fallible conversion to handle overflow
        let expiry_seconds_u64 = expiry_seconds.try_into().map_err(|_| {
            tracing::error!("OTP expiry time is too large: {} seconds", expiry_seconds);
            AuthError::Validation("OTP expiry time is too large".to_string())
        })?;

        tracing::debug!(
            "Storing OTP in Redis with key: {} and expiry: {} seconds",
            key,
            expiry_seconds_u64
        );
        let _: () = conn
            .set_ex(&key, otp, expiry_seconds_u64)
            .await
            .map_err(|e| {
                tracing::error!("Failed to store OTP in Redis: {}", e);
                AuthError::Redis(format!("Failed to store OTP: {}", e))
            })?;

        tracing::debug!("OTP stored successfully in Redis");
        Ok(())
    }

    /// Send verification email with OTP
    pub async fn send_verification_email(
        &self,
        recipient_email: &str,
        recipient_name: &str,
    ) -> Result<(), AuthError> {
        tracing::info!("Starting verification email send process");
        tracing::info!("Recipient: {} <{}>", recipient_name, recipient_email);

        let otp = self.generate_otp();
        tracing::debug!("Generated OTP: {}", otp);

        // Store OTP in Redis
        tracing::debug!("Storing OTP in Redis for email: {}", recipient_email);
        self.store_otp(recipient_email, &otp).await?;
        tracing::debug!("OTP stored successfully in Redis");

        let subject = "Verify Your Email";
        tracing::debug!("Email subject: {}", subject);

        let text_content = format!(
            r#"Hello {},

Thank you for signing up! Your verification code is:

{}

This code will expire in {} minutes.

If you didn't create an account, please ignore this email.

Best regards,
The Team"#,
            recipient_name, otp, self.otp_expiry_minutes
        );

        let html_content = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <style>
        body {{ font-family: Arial, sans-serif; line-height: 1.6; color: #333; }}
        .container {{ max-width: 600px; margin: 0 auto; padding: 20px; }}
        .header {{ background-color: #f8f9fa; padding: 20px; text-align: center; }}
        .content {{ padding: 20px; }}
        .otp {{ font-size: 32px; font-weight: bold; text-align: center; 
               padding: 20px; margin: 20px 0; background-color: #f0f0f0; 
               border-radius: 8px; letter-spacing: 8px; }}
        .footer {{ text-align: center; padding: 20px; color: #666; font-size: 14px; }}
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h2>Email Verification</h2>
        </div>
        <div class="content">
            <p>Hello {},</p>
            <p>Thank you for signing up! Please use the verification code below to complete your registration:</p>
            <div class="otp">{}</div>
            <p>This code will expire in {} minutes.</p>
            <p>If you didn't create an account, please ignore this email.</p>
        </div>
        <div class="footer">
            <p>Best regards,<br>The Team</p>
        </div>
    </div>
</body>
</html>"#,
            recipient_name, otp, self.otp_expiry_minutes
        );

        tracing::debug!("Email content prepared, calling send_email");
        tracing::debug!("Text content length: {} chars", text_content.len());
        tracing::debug!("HTML content length: {} chars", html_content.len());

        let result = self
            .send_email(
                recipient_email,
                recipient_name,
                subject,
                &html_content,
                &text_content,
            )
            .await;

        match &result {
            Ok(_) => {
                tracing::info!(
                    "Verification email sent successfully to: {}",
                    recipient_email
                );
            }
            Err(e) => {
                tracing::error!(
                    "Failed to send verification email to {}: {}",
                    recipient_email,
                    e
                );
            }
        }

        result
    }

    /// Send email
    async fn send_email(
        &self,
        recipient_email: &str,
        recipient_name: &str,
        subject: &str,
        html_content: &str,
        text_content: &str,
    ) -> Result<(), AuthError> {
        tracing::info!("Building email message");
        tracing::debug!("From: {}", self.from_email);
        tracing::debug!("To: {} <{}>", recipient_name, recipient_email);
        tracing::debug!("Subject: {}", subject);

        let email = lettre::Message::builder()
            .from(self.from_email.parse().map_err(|e| {
                tracing::error!("Invalid from email '{}': {}", self.from_email, e);
                AuthError::InternalError(format!("Invalid from email: {}", e))
            })?)
            .to(format!("{} <{}>", recipient_name, recipient_email)
                .parse()
                .map_err(|e| {
                    tracing::error!(
                        "Invalid recipient email '{} <{}>': {}",
                        recipient_name,
                        recipient_email,
                        e
                    );
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
        let mut conn = self.pool.get();

        let key = format!("{}{}", OTP_KEY_PREFIX, email);

        let stored_otp: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get OTP: {}", e)))?;

        match stored_otp {
            Some(otp) if otp == submitted_otp => {
                // Delete OTP after successful verification
                let _: () = conn
                    .del(&key)
                    .await
                    .map_err(|e| AuthError::Redis(format!("Failed to delete OTP: {}", e)))?;
                Ok(())
            }
            Some(_) => Err(AuthError::InvalidOtp),
            None => Err(AuthError::OtpExpired),
        }
    }

    /// checking if OTP exists and is not expired
    pub async fn has_valid_otp(&self, email: &str) -> Result<bool, AuthError> {
        let mut conn = self.pool.get();

        let key = format!("{}{}", OTP_KEY_PREFIX, email);

        let stored_otp: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to check OTP: {}", e)))?;

        Ok(stored_otp.is_some())
    }

    /// resend verification email, will send new OTP if expired, keeps existing if valid
    pub async fn resend_verification_email(
        &self,
        recipient_email: &str,
        recipient_name: &str,
    ) -> Result<bool, AuthError> {
        tracing::info!("Starting resend verification email process");
        tracing::info!("Recipient: {} <{}>", recipient_name, recipient_email);

        // checking if there's already a valid OTP and get it
        let mut conn = self.pool.get();

        let key = format!("{}{}", OTP_KEY_PREFIX, recipient_email);
        let existing_otp: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to check existing OTP: {}", e)))?;

        let (otp, is_new_otp) = if let Some(existing_otp) = existing_otp {
            tracing::info!("Using existing valid OTP for {}", recipient_email);
            (existing_otp, false)
        } else {
            // generating new OTP since the existing one is expired or not exist
            let new_otp = self.generate_otp();
            tracing::debug!("Generated new OTP: {}", new_otp);

            // storing new OTP in Redis
            tracing::debug!("Storing new OTP in Redis for email: {}", recipient_email);
            self.store_otp(recipient_email, &new_otp).await?;
            tracing::debug!("New OTP stored successfully in Redis");
            (new_otp, true)
        };

        let subject = if is_new_otp {
            "Verify Your Email - New Code"
        } else {
            "Verify Your Email - Resent Code"
        };
        tracing::debug!("Email subject: {}", subject);

        let code_description = if is_new_otp {
            "Your new verification code is:"
        } else {
            "Your verification code is:"
        };

        let text_content = format!(
            r#"Hello {},

{}

{}

This code will expire in {} minutes.

If you didn't request this code, please ignore this email.

Best regards,
The Team"#,
            recipient_name, code_description, otp, self.otp_expiry_minutes
        );

        let html_content = format!(
            r#"
<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Verify Your Email</title>
</head>
<body style="font-family: Arial, sans-serif; line-height: 1.6; color: #333; max-width: 600px; margin: 0 auto; padding: 20px;">
    <div style="background: #f8f9fa; padding: 30px; border-radius: 10px; text-align: center;">
        <h1 style="color: #2c3e50; margin-bottom: 20px;">Verify Your Email</h1>
        
        <p style="font-size: 16px; margin-bottom: 30px;">Hello {},</p>
        
        <div style="background: #fff; padding: 20px; border-radius: 8px; border: 2px solid #e9ecef; margin: 20px 0;">
            <p style="font-size: 14px; color: #666; margin-bottom: 10px;">{}</p>
            <div style="font-size: 32px; font-weight: bold; color: #2c3e50; letter-spacing: 5px; margin: 20px 0;">{}</div>
        </div>
        
        <p style="font-size: 14px; color: #666; margin-bottom: 20px;">
            This code will expire in <strong>{} minutes</strong>.
        </p>
        
        <p style="font-size: 14px; color: #666;">
            If you didn't request this code, please ignore this email.
        </p>
        
        <hr style="border: none; border-top: 1px solid #e9ecef; margin: 30px 0;">
        <p style="font-size: 12px; color: #999; margin: 0;">
            Best regards,<br>
            The Team
        </p>
    </div>
</body>
</html>"#,
            recipient_name, code_description, otp, self.otp_expiry_minutes
        );

        tracing::debug!("Sending verification email to: {}", recipient_email);
        let result = self
            .send_email(
                recipient_email,
                recipient_name,
                subject,
                &html_content,
                &text_content,
            )
            .await;

        match result {
            Ok(_) => {
                tracing::info!(
                    "Verification email sent successfully to: {}",
                    recipient_email
                );
                Ok(is_new_otp) // Return whether a new OTP was generated
            }
            Err(e) => {
                tracing::error!(
                    "Failed to send verification email to {}: {}",
                    recipient_email,
                    e
                );
                Err(e)
            }
        }
    }

    /// Check rate limit for password reset requests
    /// Returns true if rate limit is exceeded
    pub async fn check_password_reset_rate_limit(&self, email: &str) -> Result<bool, AuthError> {
        let mut conn = self.pool.get();

        let key = format!("{}{}", RATE_LIMIT_PASSWORD_RESET_PREFIX, email);
        let count: Option<u64> = conn
            .get(&key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get rate limit: {}", e)))?;
        let count = count.unwrap_or(0);

        // Max 3 requests per hour
        if count >= 3 {
            return Ok(true);
        }

        // Increment counter
        let _: () = conn
            .incr(&key, 1)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to increment rate limit: {}", e)))?;

        // Set TTL to 1 hour if this is the first request
        if count == 0 {
            let _: () = conn
                .expire(&key, 3600)
                .await
                .map_err(|e| AuthError::Redis(format!("Failed to set rate limit TTL: {}", e)))?;
        }

        Ok(false)
    }

    /// Check rate limit for OTP verification attempts
    /// Returns true if rate limit is exceeded
    async fn check_otp_verify_rate_limit(&self, email: &str) -> Result<bool, AuthError> {
        let mut conn = self.pool.get();

        let key = format!("{}{}", RATE_LIMIT_OTP_VERIFY_PREFIX, email);
        let count: Option<u64> = conn
            .get(&key)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get rate limit: {}", e)))?;
        let count = count.unwrap_or(0);

        // Max 5 attempts per OTP lifetime is 15 min
        if count >= 5 {
            return Ok(true);
        }

        // Increment counter
        let _: () = conn
            .incr(&key, 1)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to increment rate limit: {}", e)))?;

        // Set TTL to 15 min if this is the first attempt
        if count == 0 {
            let _: () = conn
                .expire(&key, 900) // 15 minutes
                .await
                .map_err(|e| AuthError::Redis(format!("Failed to set rate limit TTL: {}", e)))?;
        }

        Ok(false)
    }

    /// Store password reset OTP in Redis
    async fn store_password_reset_otp(&self, email: &str, otp: &str) -> Result<(), AuthError> {
        let mut conn = self.pool.get();

        let key = format!("{}{}", PASSWORD_RESET_OTP_PREFIX, email);
        // Password reset OTP expires in 15 min
        let expiry_seconds = 15 * 60; // 15 minutes
        let expiry_seconds_u64 = expiry_seconds
            .try_into()
            .map_err(|_| AuthError::Validation("OTP expiry time is too large".to_string()))?;

        let _: () = conn
            .set_ex(&key, otp, expiry_seconds_u64)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store password reset OTP: {}", e)))?;

        Ok(())
    }

    /// Send password reset OTP email
    pub async fn send_password_reset_otp(
        &self,
        recipient_email: &str,
        recipient_name: &str,
    ) -> Result<(), AuthError> {
        tracing::info!("Starting password reset OTP email send process");
        tracing::info!("Recipient: {} <{}>", recipient_name, recipient_email);

        let otp = self.generate_otp();
        tracing::debug!("Generated password reset OTP: {}", otp);

        // Store OTP in Redis
        self.store_password_reset_otp(recipient_email, &otp).await?;

        let subject = "Reset Your Password";
        let text_content = format!(
            r#"Hello {},

You requested to reset your password. Your verification code is:

{}

This code will expire in 15 minutes.

If you didn't request this, please ignore this email.

Best regards,
The Team"#,
            recipient_name, otp
        );

        let html_content = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <style>
        body {{ font-family: Arial, sans-serif; line-height: 1.6; color: #333; }}
        .container {{ max-width: 600px; margin: 0 auto; padding: 20px; }}
        .header {{ background-color: #f8f9fa; padding: 20px; text-align: center; }}
        .content {{ padding: 20px; }}
        .otp {{ font-size: 32px; font-weight: bold; text-align: center; 
               padding: 20px; margin: 20px 0; background-color: #f0f0f0; 
               border-radius: 8px; letter-spacing: 8px; }}
        .footer {{ text-align: center; padding: 20px; color: #666; font-size: 14px; }}
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h2>Reset Your Password</h2>
        </div>
        <div class="content">
            <p>Hello {},</p>
            <p>You requested to reset your password. Please use the verification code below:</p>
            <div class="otp">{}</div>
            <p>This code will expire in <strong>15 minutes</strong>.</p>
            <p>If you didn't request this, please ignore this email.</p>
        </div>
        <div class="footer">
            <p>Best regards,<br>The Team</p>
        </div>
    </div>
</body>
</html>"#,
            recipient_name, otp
        );

        let result = self
            .send_email(
                recipient_email,
                recipient_name,
                subject,
                &html_content,
                &text_content,
            )
            .await;

        match &result {
            Ok(_) => {
                tracing::info!(
                    "Password reset OTP email sent successfully to: {}",
                    recipient_email
                );
            }
            Err(e) => {
                tracing::error!(
                    "Failed to send password reset OTP email to {}: {}",
                    recipient_email,
                    e
                );
            }
        }

        result
    }

    /// Verify password reset OTP
    /// Returns Ok(()) if OTP is valid, deletes OTP (one-time use)
    pub async fn verify_password_reset_otp(
        &self,
        email: &str,
        submitted_otp: &str,
    ) -> Result<(), AuthError> {
        // Check rate limit
        if self.check_otp_verify_rate_limit(email).await? {
            return Err(AuthError::Validation(
                "Too many OTP verification attempts. Please request a new password reset."
                    .to_string(),
            ));
        }

        let mut conn = self.pool.get();

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
                let rate_limit_key = format!("{}{}", RATE_LIMIT_OTP_VERIFY_PREFIX, email);
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
        let mut conn = self.pool.get();

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
        let mut conn = self.pool.get();

        let token_key = format!("{}{}", PASSWORD_RESET_TOKEN_PREFIX, reset_token);

        let _: () = conn
            .set_ex(&token_key, user_id.to_string(), 1800) // 30 minutes
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store reset token: {}", e)))?;

        Ok(())
    }
}
