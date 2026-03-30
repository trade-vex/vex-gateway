use actix_web::{
    HttpRequest,
    cookie::{Cookie, SameSite, time::Duration},
};

#[derive(Clone, Debug)]
pub struct CookiePolicy {
    secure: bool,
    same_site: SameSite,
    domain: Option<String>,
}

impl CookiePolicy {
    pub fn from_request(req: &HttpRequest) -> Self {
        let secure = env_bool("COOKIE_SECURE").unwrap_or_else(|| {
            request_is_secure(req)
                || env_url_is_https("BASE_URL")
                || env_url_is_https("OAUTH_REDIRECT_BASE")
        });

        let same_site = env_same_site("COOKIE_SAME_SITE").unwrap_or_else(|| {
            if secure {
                SameSite::None
            } else {
                SameSite::Lax
            }
        });

        let same_site = if secure || !matches!(same_site, SameSite::None) {
            same_site
        } else {
            SameSite::Lax
        };

        Self {
            secure,
            same_site,
            domain: env_string("COOKIE_DOMAIN"),
        }
    }

    pub fn secure(&self) -> bool {
        self.secure
    }

    pub fn build_cookie(
        &self,
        name: &str,
        value: String,
        max_age_secs: i64,
        http_only: bool,
    ) -> Cookie<'static> {
        self.cookie_builder(name, value, "/", http_only, self.same_site)
            .max_age(Duration::seconds(max_age_secs))
            .finish()
    }

    pub fn build_cookie_with_path(
        &self,
        name: &str,
        value: String,
        path: &str,
        max_age_secs: i64,
        http_only: bool,
        same_site: SameSite,
    ) -> Cookie<'static> {
        self.cookie_builder(name, value, path, http_only, same_site)
            .max_age(Duration::seconds(max_age_secs))
            .finish()
    }

    pub fn build_expired_cookie(
        &self,
        name: &str,
        path: &str,
        http_only: bool,
        same_site: SameSite,
    ) -> Cookie<'static> {
        self.cookie_builder(name, String::new(), path, http_only, same_site)
            .max_age(Duration::seconds(0))
            .finish()
    }

    fn cookie_builder(
        &self,
        name: &str,
        value: String,
        path: &str,
        http_only: bool,
        same_site: SameSite,
    ) -> actix_web::cookie::CookieBuilder<'static> {
        let mut builder = Cookie::build(name.to_string(), value)
            .path(path.to_string())
            .http_only(http_only)
            .secure(self.secure)
            .same_site(same_site);

        if let Some(domain) = &self.domain {
            builder = builder.domain(domain.clone());
        }

        builder
    }
}

fn request_is_secure(req: &HttpRequest) -> bool {
    if req.connection_info().scheme() == "https" {
        return true;
    }

    req.headers()
        .get("X-Forwarded-Proto")
        .and_then(|header| header.to_str().ok())
        .map(|value| value.split(',').any(|part| part.trim() == "https"))
        .unwrap_or(false)
}

fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .and_then(|value| match value.as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
}

fn env_same_site(name: &str) -> Option<SameSite> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .and_then(|value| match value.as_str() {
            "strict" => Some(SameSite::Strict),
            "lax" => Some(SameSite::Lax),
            "none" => Some(SameSite::None),
            _ => None,
        })
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_url_is_https(name: &str) -> bool {
    env_string(name)
        .map(|value| value.starts_with("https://"))
        .unwrap_or(false)
}
