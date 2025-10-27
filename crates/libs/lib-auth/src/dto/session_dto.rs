use serde::{Deserialize, Serialize};
use crate::dto::auth_response_dto::UserInfo;

/// Session data stored in Redis
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionData {
    pub user: UserInfo,
    pub access_token: String,
    pub refresh_token: String,
}

/// Response sent to client with session ID
#[derive(Serialize)]
pub struct SessionResponse {
    pub session_id: String,
    pub message: String,
}

