# VEX Gateway Local Testing Setup

## ✅ What's Been Set Up

### Docker Services
- ✅ **PostgreSQL** - Database running in Docker (localhost:5432)
- ✅ **Redis** - Cache running in Docker (localhost:6380)
- ✅ **Kafka + Zookeeper** - Message broker for trading events
- ✅ **Kafka UI** - Web interface for Kafka (http://localhost:8080)
- ✅ **Trading Events Consumer** - Real-time event processing
- ✅ **WebSocket Server** - Real-time data streaming (ws://localhost:8765)

### Database Configuration
- ✅ Database: `trading_db`
- ✅ User: `postgres`
- ✅ Password: `postgres123`
- ✅ Port: `5432`
- ✅ Automatic migrations on startup

### Redis Configuration
- ✅ Port: `6380` (Docker mapped)
- ✅ Max memory: 2GB
- ✅ Policy: allkeys-lru

### Kafka Configuration
- ✅ Bootstrap servers: `localhost:9092`
- ✅ Auto topic creation enabled
- ✅ Trading events topic ready

### JWT Authentication
- ✅ RSA key pair generated (`crates/libs/lib-auth/src/keys/`)
- ✅ JWT configuration with proper issuer/audience
- ✅ Token TTL: 60 minutes access, 30 days refresh

## 🚀 Quick Start

**Prerequisites**: 
1. Docker services must be running (PostgreSQL, Redis, Kafka)
2. Local PostgreSQL stopped (if installed via Homebrew)
3. Aeron Media Driver must be running
4. VEX Core service must be running

```bash
# 1. Setup gateway (generate keys, build)
make setup

# 4. Start gateway server
make start
```

Gateway runs at http://localhost:8081

## 🧪 Testing Endpoints

### Public Endpoints (No Auth Required)
- `GET /health` - Health check
- `POST /signup` - User registration
- `POST /login` - User login
- `POST /refresh` - Refresh JWT token

### JWT Protected Endpoints
- `GET /profile` - Get user profile
- `POST /logout` - Logout user
- `POST /api-keys` - Create API key
- `GET /api-keys` - List API keys

### API Key Protected Endpoints
- `GET /balances` - Get user balances
- `GET /orders` - Get open orders
- `POST /orders` - Place order

## 📝 Manual Testing Examples

### 1. User Signup
```bash
curl -X POST http://localhost:8081/signup \
  -H "Content-Type: application/json" \
  -d '{
    "email": "newuser@example.com",
    "username": "newuser",
    "password": "password123"
  }'
```

### 2. User Login
```bash
curl -X POST http://localhost:8081/login \
  -H "Content-Type: application/json" \
  -d '{
    "email": "test@example.com",
    "password": "password123"
  }'
```

### 3. Get Profile (JWT Required)
```bash
curl -X GET http://localhost:8081/profile \
  -H "Authorization: Bearer YOUR_JWT_TOKEN"
```

### 4. API Key Authentication
```bash
# Generate timestamp and signature
timestamp=$(date +%s)000
query="timestamp=$timestamp"
signature=$(echo -n "$query" | openssl dgst -sha256 -hmac "test_secret_key_67890" -binary | base64)

# Make request
curl -X GET "http://localhost:8081/balances?$query&signature=$signature" \
  -H "X-API-KEY: test_api_key_12345"
```

## 🔧 Prerequisites

Docker services must be running:
- PostgreSQL (localhost:5432)
- Redis (localhost:6380)  
- Kafka (localhost:9092)

Check with: `docker ps`

## 🆘 Troubleshooting

### Gateway won't start
- Check Docker services running: `docker ps`
- Check JWT keys exist: `ls crates/libs/lib-auth/src/keys/`
- Generate keys: `make keys`

### Connection errors
- Verify PostgreSQL: `docker ps | grep postgres`
- Verify Redis: `docker ps | grep redis`
- Check config: `config/development.toml`

## 📚 Commands

```bash
make setup      # Setup gateway
make start      # Start server
make test-auth  # Test JWT auth
make test-api   # Test API key auth
make clean      # Clean build
make help       # Show all commands
```

## 🎯 Next Steps

1. **Setup gateway**: `make setup`
2. **Start server**: `make start`
3. **Test**: `make test-auth`

## 🌐 Service URLs

| Service | URL | Description |
|---------|-----|-------------|
| Gateway API | http://localhost:8081 | REST API endpoints |
| Kafka UI | http://localhost:8080 | Kafka management interface |
| WebSocket | ws://localhost:8765 | Real-time data streaming |
| PostgreSQL | localhost:5432 | Database (trading_db) |
| Redis | localhost:6380 | Cache server |
| Kafka | localhost:9092 | Message broker |

---

**Quick start: `make setup && make start`**
