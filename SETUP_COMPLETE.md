# VEX Gateway Local Testing Setup - Complete! 🎉

Your VEX Gateway authentication testing environment is now ready for manual testing.

## ✅ What's Been Set Up

### Configuration Files
- ✅ `config/development.toml` - Complete development configuration
- ✅ `.env` - Environment variables for local testing
- ✅ `env.example` - Template for environment variables

### JWT Authentication
- ✅ RSA key pair generated (`crates/libs/lib-auth/src/keys/`)
- ✅ JWT configuration with proper issuer/audience
- ✅ Token TTL: 60 minutes access, 30 days refresh

### Database Setup
- ✅ PostgreSQL configuration (localhost:5432)
- ✅ Database: `mydb`, User: `myuser`, Password: `mypassword`
- ✅ Migration scripts ready for users, api_keys, and trading tables

### Redis Setup
- ✅ Redis configuration (localhost:6379)
- ✅ Connection pooling configured

### Test Data
- ✅ Test user: `test@example.com` / `password123`
- ✅ Test API key: `test_api_key_12345` / `test_secret_key_67890`
- ✅ Test balance: 1,000,000 available

## 🚀 Quick Start Commands

### Option 1: Use Makefile (Recommended)
```bash
make setup    # Complete setup
make start    # Start server
make test-all # Run all tests
```

### Option 2: Use Scripts Directly
```bash
./scripts/setup_dev.sh      # Complete setup
./scripts/start_server.sh   # Start server
./test_scripts/test_auth.sh # Test JWT auth
```

### Option 3: Manual Commands
```bash
cargo run --bin gateway     # Start server directly
```

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

## 🔧 Prerequisites Check

Before starting, ensure these services are running:

### PostgreSQL
```bash
# Check if running
pg_isready

# Start if needed (macOS)
brew services start postgresql

# Start if needed (Ubuntu)
sudo systemctl start postgresql
```

### Redis
```bash
# Check if running
redis-cli ping

# Start if needed (macOS)
brew services start redis

# Start if needed (Ubuntu)
sudo systemctl start redis
```

## 🆘 Troubleshooting

### Server Won't Start
1. Check if port 8081 is available: `lsof -i :8081`
2. Verify PostgreSQL is running: `pg_isready`
3. Verify Redis is running: `redis-cli ping`
4. Check logs for specific errors

### Database Issues
1. Ensure PostgreSQL is running
2. Check database exists: `psql -l`
3. Verify connection: `psql -h localhost -U myuser -d mydb`

### Authentication Issues
1. Verify JWT keys exist: `ls crates/libs/lib-auth/src/keys/`
2. Check .env file exists and has correct values
3. Verify test data exists: `./scripts/verify_setup.sh`

## 📚 Available Scripts

### Setup Scripts
- `scripts/setup_dev.sh` - Complete development setup
- `scripts/generate_keys.sh` - Generate JWT keys
- `scripts/setup_database.sh` - Setup database
- `scripts/setup_redis.sh` - Setup Redis
- `scripts/setup_test_data.sh` - Add test data

### Testing Scripts
- `test_scripts/test_auth.sh` - JWT authentication tests
- `test_scripts/test_api_key.sh` - API key authentication tests

### Utility Scripts
- `scripts/start_server.sh` - Start server with checks
- `scripts/verify_setup.sh` - Verify setup completeness
- `scripts/quick_start.sh` - Show quick start guide

## 🎯 Next Steps

1. **Start the server**: `make start` or `./scripts/start_server.sh`
2. **Test JWT authentication**: `./test_scripts/test_auth.sh`
3. **Test API key authentication**: `./test_scripts/test_api_key.sh`
4. **Manual testing**: Use the curl examples above
5. **Verify setup**: `./scripts/verify_setup.sh`

## 📖 Documentation

- `TESTING_SETUP.md` - Complete setup guide
- `README.md` - Project documentation
- `Makefile` - Available commands

---

**🎉 You're all set! Start testing authentication with: `make start`**
