# lib-config

Configuration management for the vex-gateway-service.

## Usage

The configuration can be set using environment variables:

- `SERVER_HOST` - Server host (default: "127.0.0.1")
- `SERVER_PORT` - Server port (default: 8000)

## Example

```bash
# Set custom host and port
export SERVER_HOST=0.0.0.0
export SERVER_PORT=3000

# Run the application
cargo run
```

## Default Values

If no environment variables are set, the following defaults are used:
- Host: `127.0.0.1`
- Port: `8000` 