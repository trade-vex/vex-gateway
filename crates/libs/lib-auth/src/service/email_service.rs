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
        let creds = Credentials::new(
            email_config.smtp_username.clone(),
            email_config.smtp_password.expose_secret().clone(),
        );

        let smtp_transport = AsyncSmtpTransport::<Tokio1Executor>::relay(&email_config.smtp_host)
            .map_err(|e| AuthError::InternalError(format!("SMTP relay error: {}", e)))?
            .credentials(creds)
            .port(email_config.smtp_port)
            .build();

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
        // Validate otp_expiry_minutes is positive
        if self.otp_expiry_minutes <= 0 {
            return Err(AuthError::Validation(
                "OTP expiry minutes must be positive".to_string(),
            ));
        }

        let mut conn = self
            .redis_pool
            .get()
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to get Redis connection: {}", e)))?;

        let key = format!("{}{}", OTP_KEY_PREFIX, email);
        let expiry_seconds = self.otp_expiry_minutes * 60;

        // Convert to u64 using fallible conversion to handle overflow
        let expiry_seconds_u64 = expiry_seconds
            .try_into()
            .map_err(|_| AuthError::Validation("OTP expiry time is too large".to_string()))?;

        let _: () = conn
            .set_ex(&key, otp, expiry_seconds_u64)
            .await
            .map_err(|e| AuthError::Redis(format!("Failed to store OTP: {}", e)))?;

        Ok(())
    }

    /// Send verification email with OTP
    pub async fn send_verification_email(
        &self,
        recipient_email: &str,
        recipient_name: &str,
    ) -> Result<(), AuthError> {
        let otp = self.generate_otp();

        // Store OTP in Redis
        self.store_otp(recipient_email, &otp).await?;

        let subject = "Verify Your Email";

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

        self.send_email(
            recipient_email,
            recipient_name,
            subject,
            &html_content,
            &text_content,
        )
        .await
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
        let email = lettre::Message::builder()
            .from(
                self.from_email
                    .parse()
                    .map_err(|e| AuthError::InternalError(format!("Invalid from email: {}", e)))?,
            )
            .to(format!("{} <{}>", recipient_name, recipient_email)
                .parse()
                .map_err(|e| AuthError::InternalError(format!("Invalid recipient email: {}", e)))?)
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
            .map_err(|e| AuthError::InternalError(format!("Failed to build email: {}", e)))?;

        self.smtp_transport
            .send(email)
            .await
            .map_err(|e| AuthError::InternalError(format!("Failed to send email: {}", e)))?;

        Ok(())
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
