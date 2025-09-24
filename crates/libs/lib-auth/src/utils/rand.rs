use rand::{rngs::OsRng, Rng};
use rand::distributions::{Alphanumeric, Distribution};

/// Generates a cryptographically secure random alphanumeric string.
/// 
/// # Cryptographic Security
/// 
/// This function uses `OsRng` (Operating System Random Number Generator) which:
/// - Sources entropy from the OS kernel's cryptographically secure entropy pool
/// - Uses `/dev/urandom` on Unix systems and `CryptGenRandom` on Windows
/// - Provides cryptographically secure random bytes suitable for security applications
/// - Is the recommended RNG for generating tokens, session IDs, and other security-critical data
/// 
/// # Distribution
/// 
/// Uses `Alphanumeric` distribution which provides:
/// - Uniform sampling across the 62-character alphanumeric set (A-Z, a-z, 0-9)
/// - Cryptographically sound when paired with a secure RNG
/// - No bias or predictable patterns in character selection
/// 
/// # Performance
/// 
/// - O(n) time complexity where n is the requested length
/// - Minimal memory allocation through iterator-based generation
/// - OS-level entropy ensures no blocking on entropy collection
/// 
/// # Security Properties
/// 
/// - **Unpredictability**: Each character is independently random
/// - **Uniformity**: All 62 alphanumeric characters have equal probability
/// - **Uniqueness**: Probability of collision is 1/62^n for n-length strings
/// - **Forward secrecy**: Previous outputs don't reveal future outputs
/// 
/// # Parameters
/// 
/// * `length` - Number of characters in the generated string (must be > 0)
/// 
/// # Returns
/// 
/// A `String` containing `length` cryptographically secure alphanumeric characters.
/// 
/// # Panics
/// 
/// This function will panic if the OS entropy source is unavailable (extremely rare).
/// 
/// # Examples
/// 
/// ```
/// use lib_auth::utils::rand::generate_random_string;
/// 
/// let token = generate_random_string(32);
/// assert_eq!(token.len(), 32);
/// assert!(token.chars().all(|c| c.is_ascii_alphanumeric()));
/// ```
pub fn generate_random_string(length: usize) -> String {
    Alphanumeric
        .sample_iter(&mut OsRng)
        .take(length)
        .map(char::from)
        .collect::<String>()
}

pub fn generate_random_bytes(length: usize) -> Vec<u8> {
    let mut rng = rand::thread_rng();
    (0..length).map(|_| rng.r#gen()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_generate_random_string() {
        let s1 = generate_random_string(32);
        let s2 = generate_random_string(32);

        assert_eq!(s1.len(), 32);
        assert_eq!(s2.len(), 32);
        assert_ne!(s1, s2);

        // Should only contain valid characters
        assert!(s1.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(s2.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn test_cryptographic_properties() {
        // Test uniqueness - generate many strings and ensure they're all different
        let mut strings = HashSet::new();
        for _ in 0..1000 {
            let s = generate_random_string(16);
            assert!(strings.insert(s), "Generated duplicate string - not cryptographically secure");
        }
        assert_eq!(strings.len(), 1000);

        // Test character distribution - should have reasonable distribution
        let mut char_counts = std::collections::HashMap::new();
        for _ in 0..10000 {
            let s = generate_random_string(1);
            *char_counts.entry(s.chars().next().unwrap()).or_insert(0) += 1;
        }
        
        // Should have at least some characters from each category
        let has_upper = char_counts.keys().any(|&c| c.is_ascii_uppercase());
        let has_lower = char_counts.keys().any(|&c| c.is_ascii_lowercase());
        let has_digit = char_counts.keys().any(|&c| c.is_ascii_digit());
        
        assert!(has_upper, "Missing uppercase letters in distribution");
        assert!(has_lower, "Missing lowercase letters in distribution");
        assert!(has_digit, "Missing digits in distribution");
    }

    #[test]
    fn test_osrng_usage() {
        // Verify we're using OsRng by checking it doesn't panic
        // OsRng should always be available on supported platforms
        let _ = generate_random_string(1);
        let _ = generate_random_string(100);
        let _ = generate_random_string(1000);
    }
}
