use deadpool_redis::Pool;
use deadpool_redis::redis::AsyncCommands;
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor, message::header::ContentType,
    transport::smtp::authentication::Credentials,
};
use rand::Rng;
use std::sync::Arc;

use crate::config::EmailConfig;
use crate::error::auth_error::AuthError;
use secrecy::ExposeSecret;

const OTP_KEY_PREFIX: &str = "otp:verification:";

#[derive(Clone)]
pub struct EmailService {
    smtp_transport: AsyncSmtpTransport<Tokio1Executor>,
    redis_pool: Arc<Pool>,
    from_email: String,
    otp_length: usize,
    otp_expiry_minutes: i64,
}

impl EmailService {
    pub fn new(
        email_config: &EmailConfig,
        redis_pool: Arc<Pool>,
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

        tracing::debug!("Creating SMTP transport for host: {}", email_config.smtp_host);
        
        // Configure SMTP transport based on host
        let smtp_transport = if email_config.smtp_host == "localhost" || email_config.smtp_host == "127.0.0.1" {
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
                    tracing::error!("SMTP relay error for host {}: {}", email_config.smtp_host, e);
                    AuthError::InternalError(format!("SMTP relay error: {}", e))
                })?
                .credentials(creds)
                .port(email_config.smtp_port)
                .build()
        };

        tracing::info!("EmailService initialized successfully");
        Ok(Self {
            smtp_transport,
            redis_pool,
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
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| {
                tracing::error!("Failed to get Redis connection: {}", e);
                AuthError::Redis(format!("Failed to get Redis connection: {}", e))
            })?;

        let key = format!("{}{}", OTP_KEY_PREFIX, email);
        let expiry_seconds = self.otp_expiry_minutes * 60;

        tracing::debug!("Redis key: {}", key);
        tracing::debug!("OTP expiry: {} minutes ({} seconds)", self.otp_expiry_minutes, expiry_seconds);

        // Convert to u64 using fallible conversion to handle overflow
        let expiry_seconds_u64 = expiry_seconds
            .try_into()
            .map_err(|_| {
                tracing::error!("OTP expiry time is too large: {} seconds", expiry_seconds);
                AuthError::Validation("OTP expiry time is too large".to_string())
            })?;

        tracing::debug!("Storing OTP in Redis with key: {} and expiry: {} seconds", key, expiry_seconds_u64);
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

        let result = self.send_email(
            recipient_email,
            recipient_name,
            subject,
            &html_content,
            &text_content,
        ).await;

        match &result {
            Ok(_) => {
                tracing::info!("Verification email sent successfully to: {}", recipient_email);
            }
            Err(e) => {
                tracing::error!("Failed to send verification email to {}: {}", recipient_email, e);
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
            .from(
                self.from_email
                    .parse()
                    .map_err(|e| {
                        tracing::error!("Invalid from email '{}': {}", self.from_email, e);
                        AuthError::InternalError(format!("Invalid from email: {}", e))
                    })?,
            )
            .to(format!("{} <{}>", recipient_name, recipient_email)
                .parse()
                .map_err(|e| {
                    tracing::error!("Invalid recipient email '{} <{}>': {}", recipient_name, recipient_email, e);
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
        
        tracing::debug!("Starting SMTP send with {} second timeout", timeout_duration.as_secs());
        
        let result = tokio::time::timeout(timeout_duration, send_future)
            .await
            .map_err(|_| {
                let elapsed = start_time.elapsed();
                tracing::error!("SMTP send timed out after {:?} (timeout: {:?})", elapsed, timeout_duration);
                AuthError::InternalError(format!("SMTP send timed out after {:?}", timeout_duration))
            })?
            .map_err(|e| {
                let elapsed = start_time.elapsed();
                tracing::error!("SMTP send failed after {:?}: {}", elapsed, e);
                AuthError::InternalError(format!("Failed to send email: {}", e))
            });

        match &result {
            Ok(response) => {
                let elapsed = start_time.elapsed();
                tracing::info!("Email sent successfully via SMTP in {:?}, response: {:?}", elapsed, response);
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
        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

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
}
