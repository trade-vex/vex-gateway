#!/bin/bash

# Generate JWT keys for development testing
# This script creates RSA key pairs for JWT signing

set -e

# Create keys directory if it doesn't exist
mkdir -p crates/libs/lib-auth/src/keys

echo "Generating JWT keys for development..."

# Generate private key
openssl genrsa -out crates/libs/lib-auth/src/keys/private_key.pem 2048

# Generate public key from private key
openssl rsa -in crates/libs/lib-auth/src/keys/private_key.pem -pubout -out crates/libs/lib-auth/src/keys/public_key.pem

echo "✅ JWT keys generated successfully!"
echo "📁 Private key: crates/libs/lib-auth/src/keys/private_key.pem"
echo "📁 Public key: crates/libs/lib-auth/src/keys/public_key.pem"

# Set proper permissions
chmod 600 crates/libs/lib-auth/src/keys/private_key.pem
chmod 644 crates/libs/lib-auth/src/keys/public_key.pem

echo "🔒 Set proper file permissions"
