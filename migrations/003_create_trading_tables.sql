-- Create orders table
CREATE TABLE orders (
    order_id BIGSERIAL PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    price BIGINT NOT NULL,
    size BIGINT NOT NULL,
    side TEXT NOT NULL CHECK (side IN ('Bid', 'Ask')),
    timestamp BIGINT NOT NULL
);

-- Create user_balances table
CREATE TABLE user_balances (
    user_id UUID PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    available BIGINT NOT NULL DEFAULT 0,
    locked BIGINT NOT NULL DEFAULT 0,
    last_updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create indexes for performance
CREATE INDEX idx_orders_user_id ON orders(user_id);
CREATE INDEX idx_orders_timestamp ON orders(timestamp);
CREATE INDEX idx_user_balances_user_id ON user_balances(user_id);