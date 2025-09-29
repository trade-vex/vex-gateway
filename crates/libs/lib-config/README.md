# How to Use the Configuration System

## Simple Usage (Recommended)

**Just use `Config::load_auto()` - it works from anywhere:**

```rust
use lib_config::Config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // This automatically finds your config files, no matter where you run from
    let config = Config::load_auto()?;
    
    // Use your config
    let address = config.server_address()?;
    println!("Server: {}", address);
}
```

## How It Works

1. **Automatic Discovery**: `Config::load_auto()` searches for `config/` directory in:
   - Current directory
   - Parent directories (up to filesystem root)
   - Or use `APP_CONFIG_DIR` environment variable to override

2. **Environment Detection**: 
   - Reads `APP_ENV` environment variable
   - Falls back to "development" if not set

3. **Configuration Files**: Looks for:
   - `config/default.toml` (base config)
   - `config/{environment}.toml` (environment-specific)

## Examples

### Run from project root:
```bash
cargo run --bin gateway
# Finds: ./config/development.toml
```

### Run from subdirectory:
```bash
cd crates/libs/lib-config
cargo run --example basic_usage
# Finds: ../../config/development.toml (searches up)
```

### Override config directory:
```bash
export APP_CONFIG_DIR=/path/to/config
cargo run --bin bin
# Uses: /path/to/config/development.toml
```

### Change environment:
```bash
export APP_ENV=production
cargo run --bin bin
# Uses: config/production.toml
```
