//! Password validation (mirrors the Go/Socrates rules), bcrypt hashing, and
//! random token / user-code generation.

use rand::Rng;

pub const MIN_PASSWORD_LENGTH: usize = 8;
pub const MAX_PASSWORD_LENGTH: usize = 64;

/// Validate registration gates (email format + password length).
pub fn validate_credentials(email: &str, password: &str) -> Result<(), String> {
    if email.is_empty() || password.is_empty() {
        return Err("email and password are required".to_string());
    }
    if !valid_email(email) {
        return Err("invalid email format".to_string());
    }
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err("password must be at least 8 characters".to_string());
    }
    if password.len() > MAX_PASSWORD_LENGTH {
        return Err("password is too long (maximum 64 characters)".to_string());
    }
    Ok(())
}

fn valid_email(email: &str) -> bool {
    let email = crate::store::normalize_email(email);
    // Roughly `^[^\s@]+@[^\s@]+\.[^\s@]+$`
    let at = email.find('@');
    let Some(at) = at else { return false };
    let local = &email[..at];
    let rest = &email[at + 1..];
    if local.is_empty() || local.contains(' ') || local.contains('@') {
        return false;
    }
    let dot = rest.find('.');
    let Some(dot) = dot else { return false };
    let domain = &rest[..dot];
    let tld = &rest[dot + 1..];
    !domain.is_empty() && !domain.contains(' ') && !tld.is_empty() && !tld.contains(' ')
}

/// Hash a password with bcrypt (cost 10, matching Go's DefaultCost).
pub fn hash_password(password: &str) -> Result<String, String> {
    bcrypt::hash(password, 10).map_err(|e| format!("password hashing failed: {e}"))
}

/// Verify a password against a stored bcrypt hash.
pub fn check_password(hash: &str, password: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

/// n random bytes hex-encoded (crypto/rand equivalent).
pub fn random_hex(n: usize) -> String {
    let mut rng = rand::rng();
    let mut bytes = vec![0u8; n];
    rng.fill(bytes.as_mut_slice());
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Human-friendly device code like "K7Q2-XM9D" (no I/O/1/0 ambiguity).
pub fn random_user_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rng();
    let mut b = [0u8; 8];
    for slot in b.iter_mut() {
        *slot = ALPHABET[rng.random_range(0..ALPHABET.len())];
    }
    let s: String = b.iter().map(|c| *c as char).collect();
    format!("{}-{}", &s[..4], &s[4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rules() {
        assert!(validate_credentials("a@b.co", "short").is_err());
        assert!(validate_credentials("a@b.co", "longpassword").is_ok());
        assert!(validate_credentials("a@b.co", &"x".repeat(65)).is_err());
        assert!(validate_credentials("not-an-email", "password123").is_err());
        assert!(validate_credentials("User@Example.com ", "password123").is_ok());
    }

    #[test]
    fn bcrypt_roundtrip_and_go_compat() {
        let h = hash_password("password123").unwrap();
        assert!(h.starts_with("$2"));
        assert!(check_password(&h, "password123"));
        assert!(!check_password(&h, "wrong"));

        // A hash produced by Go's bcrypt ($2a$ prefix) must verify. This is
        // the well-known bcrypt hash of "password" (cost 10, $2a$ prefix).
        let go_hash = "$2a$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy";
        assert!(check_password(go_hash, "password"));
        assert!(!check_password(go_hash, "password123"));
    }

    #[test]
    fn user_code_shape() {
        for _ in 0..50 {
            let c = random_user_code();
            assert_eq!(c.len(), 9);
            assert_eq!(c.as_bytes()[4], b'-');
        }
    }

    #[test]
    fn random_hex_lengths() {
        assert_eq!(random_hex(8).len(), 16);
        assert_eq!(random_hex(16).len(), 32);
        assert_eq!(random_hex(24).len(), 48);
    }
}
