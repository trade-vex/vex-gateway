# Session Management Architecture

## Overview

Session management is implemented as a **wrapper layer** that feeds tokens to the existing JWT/API Key authentication system.

## Architecture

### Middleware Stack (Top to Bottom)

```
Request
  ↓
Session Middleware (checks session_id cookie, gets data from Redis)
  ↓
Extract access_token from session data
  ↓
Add access_token to Authorization header
  ↓
Combined Auth Middleware (JWT/API Key)
  ↓
JWT Middleware validates the token from Authorization header
  ↓
Request proceeds
```

## How It Works

### 1. **Session Middleware (Top Layer)**

**Location**: `crates/libs/lib-auth/src/middleware/session_auth.rs`

**Responsibilities**:
- Checks for `session_id` cookie in every request
- Retrieves session data from Redis using the session ID
- Extracts `access_token` from the session data
- Adds the token to `Authorization: Bearer <token>` header
- Passes control to the next middleware layer

**Key Code**:
```rust
// Check for session cookie
if let Some(cookie) = req.cookie("session_id") {
    // Get session from Redis
    let session_data = get_session_from_redis(session_id);
    
    // Extract access_token
    let access_token = session_data.access_token;
    
    // Add to Authorization header
    req.headers_mut().insert("authorization", auth_value);
}
```

### 2. **Combined Auth Middleware**

**Location**: `crates/libs/lib-server/src/combined_auth.rs`

**Responsibilities**:
- Detects authentication type based on headers:
  - `Authorization: Bearer ...` → JWT auth
  - `X-MBX-APIKEY: ...` → API Key auth
- Routes to appropriate middleware

**Key Code**:
```rust
fn detect_auth_type(req: &HttpRequest) -> AuthType {
    if req.headers().get("authorization").is_some() {
        return AuthType::Jwt;
    }
    
    if req.headers().get("X-MBX-APIKEY").is_some() {
        return AuthType::ApiKey;
    }
    
    AuthType::None
}
```

### 3. **JWT Middleware**

**Location**: `crates/libs/lib-auth/src/middleware/jwt_auth.rs`

**Responsibilities**:
- Validates JWT token from `Authorization` header
- Checks if token is blacklisted
- Extracts user ID from token claims
- Stores `AuthenticatedUser` in request extensions

## Request Flow Examples

### Example 1: Request with Session Cookie

```
1. Client sends: GET /api/profile
   Headers: Cookie: session_id=session:abc-123

2. Session Middleware:
   ✓ Finds session_id cookie
   ✓ Gets session data from Redis
   ✓ Extracts access_token
   ✓ Adds: Authorization: Bearer eyJhbGci...
   
3. Combined Auth Middleware:
   ✓ Detects Authorization header
   ✓ Chooses JWT auth
   
4. JWT Middleware:
   ✓ Validates token
   ✓ Extracts user_id
   ✓ Stores in request extensions
   
5. Handler receives authenticated user ✅
```

### Example 2: Request with Direct JWT Token

```
1. Client sends: GET /api/profile
   Headers: Authorization: Bearer eyJhbGci...

2. Session Middleware:
   ✗ No session_id cookie
   ✓ Passes request through unchanged

3. Combined Auth Middleware:
   ✓ Detects Authorization header
   ✓ Chooses JWT auth
   
4. JWT Middleware:
   ✓ Validates token
   ✓ Extracts user_id
   
5. Handler receives authenticated user ✅
```

### Example 3: Request with API Key

```
1. Client sends: GET /api/order
   Headers: X-MBX-APIKEY: abc123...

2. Session Middleware:
   ✗ No session_id cookie
   ✓ Passes request through unchanged

3. Combined Auth Middleware:
   ✓ Detects X-MBX-APIKEY header
   ✓ Chooses API Key auth
   
4. API Key Middleware:
   ✓ Validates API key
   
5. Handler receives authenticated request ✅
```

## Benefits of This Architecture

### ✅ **Separation of Concerns**
- Session middleware: Handles session storage and token extraction
- Combined middleware: Routes to appropriate auth method
- JWT middleware: Validates tokens

### ✅ **Reuses Existing Code**
- JWT middleware validation logic is reused
- API Key middleware continues to work
- No duplication of validation logic

### ✅ **Backward Compatible**
- Direct JWT tokens still work
- API keys still work
- Sessions provide an additional authentication method

### ✅ **Unified User Identity**
- All auth methods end up with `AuthenticatedUser` in request extensions
- Handlers don't need to know which auth method was used

### ✅ **Flexible and Extensible**
- Easy to add new auth methods
- Each layer has a single responsibility
- Middleware can be easily tested in isolation

## Request Lifecycle

```
Client Request
    ↓
┌─────────────────────────────────────┐
│  Session Middleware                 │
│  - Check session_id cookie          │
│  - Get data from Redis              │
│  - Extract access_token             │
│  - Add to Authorization header      │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│  Combined Auth Middleware           │
│  - Detect auth type                 │
│  - Route to JWT or API Key          │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│  JWT Middleware (if JWT detected)   │
│  - Validate token                   │
│  - Check blacklist                  │
│  - Extract user_id                  │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│  Handler                            │
│  - Get AuthenticatedUser            │
│  - Process request                  │
└─────────────────────────────────────┘
    ↓
Response to Client
```

## Key Design Decisions

1. **Session middleware doesn't authenticate directly** - it extracts tokens and lets existing auth handle validation
2. **All auth methods converge to the same user representation** - `AuthenticatedUser`
3. **Layered approach** - each middleware has one clear responsibility
4. **No breaking changes** - existing JWT and API key flows continue to work

## Files Modified

- `crates/libs/lib-auth/src/middleware/session_auth.rs` - Session wrapper middleware
- `crates/libs/lib-auth/src/service/session_service.rs` - Session data management
- `crates/libs/lib-server/src/combined_auth.rs` - Auth routing (simplified)
- `crates/libs/lib-server/src/app.rs` - Middleware wiring
- `crates/libs/lib-auth/src/handlers/auth.rs` - Session creation in auth handlers
- `crates/libs/lib-auth/src/handlers/oauth.rs` - Session creation in OAuth handlers

