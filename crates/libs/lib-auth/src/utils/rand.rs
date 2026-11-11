use rand::distributions::{Alphanumeric, Distribution};
use rand::rngs::OsRng;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
