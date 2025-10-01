-- Create enum types for API key management
CREATE TYPE security_type AS ENUM ('none', 'trade', 'user_data', 'user_stream');
CREATE TYPE key_material_type AS ENUM ('hmac_secret', 'rsa_public_key', 'ed25519_public_key');

-- Create api_keys table
CREATE TABLE api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    api_key VARCHAR(255) NOT NULL UNIQUE,
    key_material_type key_material_type,
    key_material_value TEXT, -- Encrypted key material
    permissions security_type[] NOT NULL DEFAULT ARRAY['none']::security_type[],
    recv_window BIGINT NOT NULL DEFAULT 5000,
    enabled BOOLEAN NOT NULL DEFAULT true,
    description TEXT,
    ip_blacklist TEXT[] DEFAULT ARRAY[]::TEXT[],
    user_id UUID, -- Optional foreign key to users table
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ
);

-- Create indexes for performance
CREATE INDEX idx_api_keys_api_key ON api_keys(api_key);
CREATE INDEX idx_api_keys_user_id ON api_keys(user_id);
CREATE INDEX idx_api_keys_enabled ON api_keys(enabled);
CREATE INDEX idx_api_keys_expires_at ON api_keys(expires_at);
CREATE INDEX idx_api_keys_created_at ON api_keys(created_at);
CREATE INDEX idx_api_keys_last_used_at ON api_keys(last_used_at);

-- Partial index for active keys
CREATE INDEX idx_api_keys_active ON api_keys(api_key) 
WHERE enabled = true AND (expires_at IS NULL OR expires_at > NOW());

-- Create api_key_usage_logs table for audit and rate limiting
CREATE TABLE api_key_usage_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    api_key_id UUID NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
    endpoint VARCHAR(255) NOT NULL,
    method VARCHAR(10) NOT NULL,
    ip_address INET,
    user_agent TEXT,
    request_timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    response_status INTEGER,
    processing_time_ms INTEGER,
    request_size_bytes INTEGER,
    response_size_bytes INTEGER
);

-- Indexes for usage logs
CREATE INDEX idx_api_key_usage_logs_api_key_id ON api_key_usage_logs(api_key_id);
CREATE INDEX idx_api_key_usage_logs_timestamp ON api_key_usage_logs(request_timestamp);
CREATE INDEX idx_api_key_usage_logs_endpoint ON api_key_usage_logs(endpoint);
CREATE INDEX idx_api_key_usage_logs_ip_address ON api_key_usage_logs(ip_address);

-- Composite index for rate limiting queries
CREATE INDEX idx_api_key_usage_rate_limiting ON api_key_usage_logs(api_key_id, request_timestamp);

-- Trigger for updating updated_at column on api_keys
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ language 'plpgsql';

CREATE TRIGGER update_api_keys_updated_at 
BEFORE UPDATE ON api_keys 
FOR EACH ROW 
EXECUTE FUNCTION update_updated_at_column();

-- Function to update last_used_at when API key is used
CREATE OR REPLACE FUNCTION update_api_key_last_used()
RETURNS TRIGGER AS $$
BEGIN
    UPDATE api_keys 
    SET last_used_at = NEW.request_timestamp
    WHERE id = NEW.api_key_id;
    RETURN NEW;
END;
$$ language 'plpgsql';

CREATE TRIGGER update_api_key_last_used_trigger
AFTER INSERT ON api_key_usage_logs
FOR EACH ROW
EXECUTE FUNCTION update_api_key_last_used();
