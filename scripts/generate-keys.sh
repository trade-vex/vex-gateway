#!/bin/bash

# Generate JWT keys for development
set -e

KEYS_DIR="./keys"
PRIVATE_KEY="$KEYS_DIR/private.pem"
PUBLIC_KEY="$KEYS_DIR/public.pem"

# Create keys directory if it doesn't exist
mkdir -p "$KEYS_DIR"

# Generate private key
openssl genrsa -out "$PRIVATE_KEY" 2048

# Generate public key from private key
openssl rsa -in "$PRIVATE_KEY" -pubout -out "$PUBLIC_KEY"

# Set appropriate permissions
chmod 600 "$PRIVATE_KEY"
chmod 644 "$PUBLIC_KEY"

echo "JWT keys generated successfully:"
echo "Private key: $PRIVATE_KEY"
echo "Public key: $PUBLIC_KEY"
echo ""
echo "You can now start the application with: docker-compose up"
