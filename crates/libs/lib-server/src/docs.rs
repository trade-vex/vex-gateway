#![allow(dead_code)]

use actix_web::{HttpResponse, web};
use serde::{Deserialize, Serialize};
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::OpenApi as OpenApiDocument;
use utoipa::{IntoParams, Modify, OpenApi, ToSchema};

const DOCS_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>VEX Gateway API Docs</title>
    <style>
      body { margin: 0; padding: 0; }
      .fallback {
        font-family: ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        max-width: 720px;
        margin: 64px auto;
        padding: 0 24px;
        line-height: 1.5;
      }
      .fallback code {
        background: #f5f5f5;
        padding: 2px 4px;
        border-radius: 4px;
      }
    </style>
  </head>
  <body>
    <redoc spec-url="/openapi.json"></redoc>
    <div class="fallback">
      <noscript>
        This documentation page requires JavaScript. Use <code>/openapi.json</code> directly if needed.
      </noscript>
    </div>
    <script src="/docs-assets/redoc.standalone.js"></script>
  </body>
</html>
"#;

const REDOC_STANDALONE_JS: &str = include_str!("../assets/redoc.standalone.js");

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/openapi.json", web::get().to(openapi_json))
        .route("/docs", web::get().to(docs_ui))
        .route(
            "/docs-assets/redoc.standalone.js",
            web::get().to(redoc_standalone_js),
        );
}

async fn openapi_json() -> HttpResponse {
    HttpResponse::Ok().json(ApiDoc::openapi())
}

async fn docs_ui() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(DOCS_HTML)
}

async fn redoc_standalone_js() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("application/javascript; charset=utf-8")
        .body(REDOC_STANDALONE_JS)
}

#[derive(OpenApi)]
#[openapi(
    paths(
        health_check_doc,
        signup_doc,
        login_doc,
        refresh_token_doc,
        refresh_session_tokens_doc,
        renew_session_doc,
        check_email_doc,
        resend_verification_doc,
        verify_email_with_otp_doc,
        forgot_password_doc,
        verify_reset_otp_doc,
        reset_password_doc,
        oauth_providers_doc,
        oauth_initiate_doc,
        oauth_callback_doc,
        oauth_exchange_doc,
        logout_doc,
        profile_doc,
        change_password_doc,
        refresh_session_doc,
        session_check_doc,
        create_api_key_doc,
        list_api_keys_doc,
        get_api_key_doc,
        update_api_key_doc,
        delete_api_key_doc,
        toggle_api_key_doc,
        place_order_doc,
        cancel_order_doc,
        balances_doc,
        open_orders_doc,
        order_history_doc,
        portfolio_doc,
        balance_chart_doc,
        pnl_chart_doc,
        favorite_markets_doc,
        toggle_favorite_market_doc
    ),
    components(
        schemas(
            ErrorResponse,
            MessageResponse,
            TimestampedMessageResponse,
            EmptyObjectResponse,
            AuthenticationStatusResponse,
            UserInfo,
            UserProfileResponse,
            SignupRequest,
            SignupResponse,
            LoginRequest,
            LoginResponse,
            RefreshTokenRequest,
            RefreshTokenResponse,
            CheckEmailRequest,
            CheckEmailResponse,
            VerifyEmailRequest,
            ResendVerificationResponse,
            ChangePasswordRequest,
            ForgotPasswordRequest,
            VerifyResetOtpRequest,
            VerifyResetOtpResponse,
            ResetPasswordRequest,
            OAuthProvidersResponse,
            OAuthExchangeRequest,
            SessionRenewRequest,
            ApiKeyPermission,
            CreateApiKeyRequest,
            UpdateApiKeyRequest,
            ApiKeyResponse,
            ApiKeyListResponse,
            CreateApiKeyResponse,
            OrderSide,
            TimeInForce,
            OrderType,
            PlaceOrderRequest,
            CancelOrderRequest,
            OrderCommandResponse,
            UserBalanceResponse,
            BalancesListResponse,
            OpenOrderResponse,
            OrdersListResponse,
            OrderHistoryResponse,
            OrdersHistoryListResponse,
            PortfolioMetricsResponse,
            BalancePoint,
            BalanceChartResponse,
            PnlPoint,
            PnlChartResponse,
            FavoriteMarketsListResponse,
            ToggleFavoriteMarketRequest,
            ToggleFavoriteMarketResponse
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "System", description = "Service status and diagnostics."),
        (name = "Auth", description = "Email/password authentication, sessions, and account recovery."),
        (name = "OAuth", description = "OAuth login flows for web and mobile clients."),
        (name = "API Keys", description = "API key lifecycle management for signed API access."),
        (name = "Orders", description = "Trading order placement and cancellation."),
        (name = "Account", description = "Authenticated account data including balances, orders, charts, and favorites.")
    )
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut OpenApiDocument) {
        let description = r#"
# VEX Gateway API

This OpenAPI document is generated from the service and is intended to be the canonical reference for the mounted HTTP API.

## Authentication

Protected endpoints accept one of these authentication modes:

- `Bearer` JWT in the `Authorization` header.
- Browser session cookies managed by the auth endpoints.
- API key authentication using `X-MBX-APIKEY`.

## Signed API key requests

When a protected endpoint is called with an API key, the request must also satisfy the gateway signature rules:

- include `X-MBX-APIKEY`
- include `timestamp`
- include `signature`
- optionally include `recvWindow`

The gateway validates signatures using the same sorted-parameter payload rules implemented by the middleware. For JSON bodies, the raw JSON payload is used during verification.

## Notes

- All routes are mounted under `/api`.
- IDs that exceed JavaScript safe integer range are serialized as strings.
- Session-oriented endpoints also set or clear cookies and emit auth metadata headers.
"#;

        openapi.info.description = Some(description.to_string());

        let components = openapi.components.get_or_insert_with(Default::default);

        components.add_security_scheme(
            "bearer_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
        components.add_security_scheme(
            "session_cookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("session_id"))),
        );
        components.add_security_scheme(
            "api_key_header",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-MBX-APIKEY"))),
        );
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct ErrorResponse {
    error: String,
    #[schema(nullable = true)]
    message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct MessageResponse {
    message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct TimestampedMessageResponse {
    message: String,
    timestamp: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct EmptyObjectResponse {}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct AuthenticationStatusResponse {
    status: String,
    message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct UserInfo {
    user_id: String,
    email: String,
    username: String,
    #[schema(nullable = true)]
    avatar_url: Option<String>,
    auth_provider: String,
    email_verified: bool,
    created_at: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct UserProfileResponse {
    user_id: String,
    email: String,
    username: String,
    #[schema(nullable = true)]
    avatar_url: Option<String>,
    auth_provider: String,
    email_verified: bool,
    created_at: String,
    #[schema(nullable = true)]
    last_login: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct SignupRequest {
    email: String,
    username: String,
    password: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct SignupResponse {
    user: UserInfo,
    message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct LoginResponse {
    user_info: UserInfo,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct RefreshTokenRequest {
    refresh_token: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct RefreshTokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct CheckEmailRequest {
    email: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct CheckEmailResponse {
    exists: bool,
    email_verified: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct VerifyEmailRequest {
    email: String,
    otp: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct ResendVerificationResponse {
    message: String,
    new_code_sent: bool,
    timestamp: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct ChangePasswordRequest {
    old_password: String,
    new_password: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct ForgotPasswordRequest {
    email: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct VerifyResetOtpRequest {
    email: String,
    otp: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct VerifyResetOtpResponse {
    reset_token: String,
    message: String,
    timestamp: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct ResetPasswordRequest {
    reset_token: String,
    new_password: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct OAuthProvidersResponse {
    providers: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct OAuthExchangeRequest {
    login_code: String,
    verifier: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct SessionRenewRequest {
    #[schema(nullable = true)]
    refresh_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum ApiKeyPermission {
    None,
    Trade,
    UserData,
    UserStream,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct CreateApiKeyRequest {
    #[schema(nullable = true)]
    description: Option<String>,
    permissions: Vec<ApiKeyPermission>,
    #[schema(nullable = true)]
    recv_window: Option<u64>,
    #[schema(nullable = true)]
    expires_at: Option<String>,
    #[schema(nullable = true)]
    ip_blacklist: Option<Vec<String>>,
    #[schema(nullable = true)]
    key_material_type: Option<String>,
    #[schema(nullable = true)]
    key_material_value: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct UpdateApiKeyRequest {
    permissions: Vec<ApiKeyPermission>,
    #[schema(nullable = true)]
    ip_blacklist: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct ApiKeyResponse {
    id: String,
    api_key: String,
    permissions: Vec<ApiKeyPermission>,
    recv_window: i64,
    enabled: bool,
    #[schema(nullable = true)]
    description: Option<String>,
    ip_blacklist: Vec<String>,
    #[schema(nullable = true)]
    user_id: Option<String>,
    created_at: String,
    updated_at: String,
    #[schema(nullable = true)]
    expires_at: Option<String>,
    #[schema(nullable = true)]
    last_used_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct ApiKeyListResponse {
    api_keys: Vec<ApiKeyResponse>,
    total: usize,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct CreateApiKeyResponse {
    api_key: String,
    secret_key: String,
    key_info: ApiKeyResponse,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
enum OrderSide {
    Bid,
    Ask,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
enum TimeInForce {
    Gtc,
    Ioc,
    Fok,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
enum OrderType {
    Limit,
    Market,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct PlaceOrderRequest {
    price: u64,
    volume: u64,
    market_id: u32,
    side: OrderSide,
    time_in_force: TimeInForce,
    order_type: OrderType,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct CancelOrderRequest {
    order_id: String,
    market_id: u32,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct OrderCommandResponse {
    order_id: String,
    client_order_id: String,
    status: String,
    message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct UserBalanceResponse {
    user_id: String,
    asset_id: u32,
    available: String,
    locked: String,
    total: String,
    timestamp: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct BalancesListResponse {
    balances: Vec<UserBalanceResponse>,
    total_assets: usize,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct OpenOrderResponse {
    order_id: String,
    user_id: String,
    market_id: i32,
    price: String,
    size: String,
    side: OrderSide,
    timestamp: i64,
    time_in_force: String,
    status: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct OrdersListResponse {
    orders: Vec<OpenOrderResponse>,
    total: usize,
    page: usize,
    limit: usize,
    total_pages: usize,
    has_next: bool,
    has_prev: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct OrderHistoryResponse {
    order_id: String,
    user_id: String,
    market_id: i32,
    price: String,
    size: String,
    filled: String,
    side: OrderSide,
    timestamp: i64,
    time_in_force: String,
    status: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct OrdersHistoryListResponse {
    orders: Vec<OrderHistoryResponse>,
    total: usize,
    page: usize,
    limit: usize,
    total_pages: usize,
    has_next: bool,
    has_prev: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct PortfolioMetricsResponse {
    balance: String,
    balance_24h_ago: String,
    volume_24h: String,
    pnl_7d: String,
    max_drawdown_1m: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct BalancePoint {
    timestamp: i64,
    balance: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct BalanceChartResponse {
    balance_series: Vec<BalancePoint>,
    resolution: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct PnlPoint {
    timestamp: i64,
    pnl: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct PnlChartResponse {
    pnl_series: Vec<PnlPoint>,
    resolution: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct FavoriteMarketsListResponse {
    market_ids: Vec<u32>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct ToggleFavoriteMarketRequest {
    market_id: u32,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
struct ToggleFavoriteMarketResponse {
    success: bool,
    action: String,
    market_id: u32,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct OrdersQueryParams {
    page: Option<usize>,
    limit: Option<usize>,
    market_id: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct ResolutionQueryParams {
    resolution: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/health",
    tag = "System",
    responses((status = 200, description = "Service is healthy"))
)]
fn health_check_doc() {}

#[utoipa::path(
    post,
    path = "/api/signup",
    tag = "Auth",
    request_body = SignupRequest,
    responses(
        (status = 201, description = "User created and verification initiated", body = SignupResponse),
        (status = 400, description = "Invalid signup payload", body = ErrorResponse),
        (status = 409, description = "Email already exists", body = ErrorResponse)
    )
)]
fn signup_doc() {}

#[utoipa::path(
    post,
    path = "/api/login",
    tag = "Auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login succeeded. Session cookies and auth headers are also set.", body = LoginResponse),
        (status = 401, description = "Invalid credentials", body = ErrorResponse),
        (status = 400, description = "Invalid login payload", body = ErrorResponse)
    )
)]
fn login_doc() {}

#[utoipa::path(
    post,
    path = "/api/refresh",
    tag = "Auth",
    request_body = RefreshTokenRequest,
    responses(
        (status = 200, description = "Access token refreshed", body = RefreshTokenResponse),
        (status = 401, description = "Refresh token rejected", body = ErrorResponse)
    )
)]
fn refresh_token_doc() {}

#[utoipa::path(
    post,
    path = "/api/session/refresh-tokens",
    tag = "Auth",
    responses(
        (status = 200, description = "Session tokens rotated using the session cookie", body = MessageResponse),
        (status = 400, description = "Session cookie missing", body = ErrorResponse),
        (status = 401, description = "Session invalid", body = ErrorResponse)
    )
)]
fn refresh_session_tokens_doc() {}

#[utoipa::path(
    post,
    path = "/api/session/renew",
    tag = "Auth",
    request_body = SessionRenewRequest,
    responses(
        (status = 200, description = "Session renewed. Fresh cookies and auth headers are also set.", body = MessageResponse),
        (status = 400, description = "Refresh token missing", body = ErrorResponse),
        (status = 401, description = "Refresh token invalid", body = ErrorResponse)
    )
)]
fn renew_session_doc() {}

#[utoipa::path(
    post,
    path = "/api/check-email",
    tag = "Auth",
    request_body = CheckEmailRequest,
    responses(
        (status = 200, description = "Email existence check completed", body = CheckEmailResponse),
        (status = 400, description = "Invalid email payload", body = ErrorResponse)
    )
)]
fn check_email_doc() {}

#[utoipa::path(
    post,
    path = "/api/resend-verification",
    tag = "Auth",
    request_body = CheckEmailRequest,
    responses(
        (status = 200, description = "Verification email sent or resent", body = ResendVerificationResponse),
        (status = 404, description = "User not found", body = ErrorResponse),
        (status = 400, description = "Email already verified or payload invalid", body = ErrorResponse)
    )
)]
fn resend_verification_doc() {}

#[utoipa::path(
    post,
    path = "/api/verify-email",
    tag = "Auth",
    request_body = VerifyEmailRequest,
    responses(
        (status = 200, description = "Email verified and authenticated session established", body = LoginResponse),
        (status = 400, description = "OTP invalid or email already verified", body = ErrorResponse),
        (status = 404, description = "User not found", body = ErrorResponse)
    )
)]
fn verify_email_with_otp_doc() {}

#[utoipa::path(
    post,
    path = "/api/forgot-password",
    tag = "Auth",
    request_body = ForgotPasswordRequest,
    responses(
        (status = 200, description = "Reset email flow initiated", body = TimestampedMessageResponse),
        (status = 400, description = "Invalid email payload or rate limited", body = ErrorResponse)
    )
)]
fn forgot_password_doc() {}

#[utoipa::path(
    post,
    path = "/api/verify-reset-otp",
    tag = "Auth",
    request_body = VerifyResetOtpRequest,
    responses(
        (status = 200, description = "Password reset OTP verified", body = VerifyResetOtpResponse),
        (status = 400, description = "OTP invalid or unavailable for this account", body = ErrorResponse),
        (status = 404, description = "User not found", body = ErrorResponse)
    )
)]
fn verify_reset_otp_doc() {}

#[utoipa::path(
    post,
    path = "/api/reset-password",
    tag = "Auth",
    request_body = ResetPasswordRequest,
    responses(
        (status = 200, description = "Password reset completed", body = TimestampedMessageResponse),
        (status = 400, description = "Reset token invalid or payload invalid", body = ErrorResponse),
        (status = 404, description = "User not found", body = ErrorResponse)
    )
)]
fn reset_password_doc() {}

#[utoipa::path(
    get,
    path = "/api/oauth/providers",
    tag = "OAuth",
    responses((status = 200, description = "Available OAuth providers", body = OAuthProvidersResponse))
)]
fn oauth_providers_doc() {}

#[utoipa::path(
    get,
    path = "/api/oauth/{provider}",
    tag = "OAuth",
    params(
        ("provider" = String, Path, description = "OAuth provider identifier"),
        ("platform" = Option<String>, Query, description = "Client platform: `web` or `app`"),
        ("app_challenge" = Option<String>, Query, description = "SHA256 verifier hash required for app flows")
    ),
    responses(
        (status = 302, description = "Redirects to the provider authorization page"),
        (status = 400, description = "Required OAuth query parameters missing", body = ErrorResponse)
    )
)]
fn oauth_initiate_doc() {}

#[utoipa::path(
    get,
    path = "/api/oauth/{provider}/callback",
    tag = "OAuth",
    params(
        ("provider" = String, Path, description = "OAuth provider identifier"),
        ("code" = Option<String>, Query, description = "Authorization code returned by the provider"),
        ("state" = String, Query, description = "CSRF state or mobile flow state"),
        ("error" = Option<String>, Query, description = "Provider error code"),
        ("error_description" = Option<String>, Query, description = "Provider error details")
    ),
    responses(
        (status = 302, description = "Redirects back to the configured frontend with cookies or a login_code"),
        (status = 400, description = "OAuth callback rejected", body = ErrorResponse),
        (status = 401, description = "OAuth state mismatch", body = ErrorResponse)
    )
)]
fn oauth_callback_doc() {}

#[utoipa::path(
    post,
    path = "/api/oauth/exchange",
    tag = "OAuth",
    request_body = OAuthExchangeRequest,
    responses(
        (status = 200, description = "Mobile login code exchanged for session credentials", body = LoginResponse),
        (status = 400, description = "Login code or verifier invalid", body = ErrorResponse)
    )
)]
fn oauth_exchange_doc() {}

#[utoipa::path(
    post,
    path = "/api/logout",
    tag = "Auth",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    responses(
        (status = 200, description = "Logged out and cookies cleared", body = TimestampedMessageResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse)
    )
)]
fn logout_doc() {}

#[utoipa::path(
    get,
    path = "/api/profile",
    tag = "Auth",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    responses(
        (status = 200, description = "Authenticated user profile", body = UserProfileResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 404, description = "User not found", body = ErrorResponse)
    )
)]
fn profile_doc() {}

#[utoipa::path(
    post,
    path = "/api/change-password",
    tag = "Auth",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    request_body = ChangePasswordRequest,
    responses(
        (status = 200, description = "Password changed and sessions rotated", body = TimestampedMessageResponse),
        (status = 400, description = "Invalid password payload", body = ErrorResponse),
        (status = 401, description = "Authentication or current password invalid", body = ErrorResponse),
        (status = 404, description = "User not found", body = ErrorResponse)
    )
)]
fn change_password_doc() {}

#[utoipa::path(
    post,
    path = "/api/session/refresh",
    tag = "Auth",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    responses(
        (status = 200, description = "Session TTL refreshed", body = MessageResponse),
        (status = 400, description = "Session cookie missing", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse)
    )
)]
fn refresh_session_doc() {}

#[utoipa::path(
    get,
    path = "/api/session-check",
    tag = "Auth",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    responses(
        (status = 200, description = "Authentication succeeded", body = AuthenticationStatusResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse)
    )
)]
fn session_check_doc() {}

#[utoipa::path(
    post,
    path = "/api/api-key",
    tag = "API Keys",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    request_body = CreateApiKeyRequest,
    responses(
        (status = 201, description = "API key created. Plaintext secret is only returned once.", body = CreateApiKeyResponse),
        (status = 400, description = "Invalid API key request", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse)
    )
)]
fn create_api_key_doc() {}

#[utoipa::path(
    get,
    path = "/api/api-key",
    tag = "API Keys",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    responses(
        (status = 200, description = "Current user's API keys", body = ApiKeyListResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse)
    )
)]
fn list_api_keys_doc() {}

#[utoipa::path(
    get,
    path = "/api/api-key/{id}",
    tag = "API Keys",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    params(("id" = String, Path, description = "API key UUID")),
    responses(
        (status = 200, description = "API key details", body = ApiKeyResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 403, description = "Key belongs to another user", body = ErrorResponse),
        (status = 404, description = "API key not found", body = ErrorResponse)
    )
)]
fn get_api_key_doc() {}

#[utoipa::path(
    put,
    path = "/api/api-key/{id}",
    tag = "API Keys",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    params(("id" = String, Path, description = "API key UUID")),
    request_body = UpdateApiKeyRequest,
    responses(
        (status = 200, description = "API key updated", body = ApiKeyResponse),
        (status = 400, description = "Invalid API key update", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 403, description = "Key belongs to another user", body = ErrorResponse),
        (status = 404, description = "API key not found", body = ErrorResponse)
    )
)]
fn update_api_key_doc() {}

#[utoipa::path(
    delete,
    path = "/api/api-key/{id}",
    tag = "API Keys",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    params(("id" = String, Path, description = "API key UUID")),
    responses(
        (status = 200, description = "API key deleted or disabled", body = MessageResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 403, description = "Key belongs to another user", body = ErrorResponse),
        (status = 404, description = "API key not found", body = ErrorResponse)
    )
)]
fn delete_api_key_doc() {}

#[utoipa::path(
    patch,
    path = "/api/api-key/{id}/toggle",
    tag = "API Keys",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    params(("id" = String, Path, description = "API key UUID")),
    responses(
        (status = 200, description = "API key enabled state toggled", body = ApiKeyResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 403, description = "Key belongs to another user", body = ErrorResponse),
        (status = 404, description = "API key not found", body = ErrorResponse)
    )
)]
fn toggle_api_key_doc() {}

#[utoipa::path(
    post,
    path = "/api/order",
    tag = "Orders",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    request_body = PlaceOrderRequest,
    responses(
        (status = 200, description = "Order accepted or synchronously processed", body = OrderCommandResponse),
        (status = 400, description = "Invalid order payload", body = ErrorResponse),
        (status = 408, description = "Order processing timeout", body = OrderCommandResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "Failed to submit order to core", body = OrderCommandResponse)
    )
)]
fn place_order_doc() {}

#[utoipa::path(
    post,
    path = "/api/cancel",
    tag = "Orders",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    request_body = CancelOrderRequest,
    responses(
        (status = 200, description = "Cancel request processed", body = OrderCommandResponse),
        (status = 400, description = "Invalid cancel payload", body = ErrorResponse),
        (status = 408, description = "Cancel timeout", body = OrderCommandResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "Failed to submit cancel to core", body = OrderCommandResponse)
    )
)]
fn cancel_order_doc() {}

#[utoipa::path(
    get,
    path = "/api/balances",
    tag = "Account",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    responses(
        (status = 200, description = "Current balances", body = BalancesListResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "Balance retrieval failed", body = ErrorResponse)
    )
)]
fn balances_doc() {}

#[utoipa::path(
    get,
    path = "/api/orders/open",
    tag = "Account",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    params(OrdersQueryParams),
    responses(
        (status = 200, description = "Open orders", body = OrdersListResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "Order retrieval failed", body = ErrorResponse)
    )
)]
fn open_orders_doc() {}

#[utoipa::path(
    get,
    path = "/api/orders/history",
    tag = "Account",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    params(OrdersQueryParams),
    responses(
        (status = 200, description = "Historical orders", body = OrdersHistoryListResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "Order history retrieval failed", body = ErrorResponse)
    )
)]
fn order_history_doc() {}

#[utoipa::path(
    get,
    path = "/api/portfolio",
    tag = "Account",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    responses(
        (status = 200, description = "Portfolio metrics snapshot", body = PortfolioMetricsResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "Portfolio retrieval failed", body = ErrorResponse)
    )
)]
fn portfolio_doc() {}

#[utoipa::path(
    get,
    path = "/api/balance-chart",
    tag = "Account",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    params(ResolutionQueryParams),
    responses(
        (status = 200, description = "Balance time series", body = BalanceChartResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "Balance chart retrieval failed", body = ErrorResponse)
    )
)]
fn balance_chart_doc() {}

#[utoipa::path(
    get,
    path = "/api/pnl-chart",
    tag = "Account",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    params(ResolutionQueryParams),
    responses(
        (status = 200, description = "PnL time series", body = PnlChartResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "PnL chart retrieval failed", body = ErrorResponse)
    )
)]
fn pnl_chart_doc() {}

#[utoipa::path(
    get,
    path = "/api/favorites",
    tag = "Account",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    responses(
        (status = 200, description = "Favorite market identifiers", body = FavoriteMarketsListResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "Favorite market retrieval failed", body = ErrorResponse)
    )
)]
fn favorite_markets_doc() {}

#[utoipa::path(
    post,
    path = "/api/favorites",
    tag = "Account",
    security(("bearer_auth" = []), ("session_cookie" = []), ("api_key_header" = [])),
    request_body = ToggleFavoriteMarketRequest,
    responses(
        (status = 200, description = "Favorite market state toggled", body = ToggleFavoriteMarketResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "Favorite market toggle failed", body = ErrorResponse)
    )
)]
fn toggle_favorite_market_doc() {}
