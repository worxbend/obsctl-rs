use base64::Engine as _;
use sha2::{Digest, Sha256};

pub fn compute_authentication(password: &str, salt: &str, challenge: &str) -> String {
    let secret = {
        let mut h = Sha256::new();
        h.update(password.as_bytes());
        h.update(salt.as_bytes());
        base64::engine::general_purpose::STANDARD.encode(h.finalize())
    };
    {
        let mut h = Sha256::new();
        h.update(secret.as_bytes());
        h.update(challenge.as_bytes());
        base64::engine::general_purpose::STANDARD.encode(h.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_known_vector() {
        // Verify algorithm: secret = base64(sha256(password + salt)),
        // authentication = base64(sha256(secret + challenge)).
        let password = "supersecretpassword";
        let salt = "PZVbYpvAnZut2SS3k3tnTQ==";
        let challenge = "lfYW3AhFLp2YcILmwSQ9rSFRIiEQgxuEk5hSyQ3XGaQ=";
        let result = compute_authentication(password, salt, challenge);
        // Value derived from our SHA-256 implementation; must match obs-websocket 5.x auth spec.
        assert_eq!(result, "KyqYIxIYmV+kMWMia3ahAvmhvF16ReqnQK6KLN9onU4=");
    }

    #[test]
    fn auth_is_deterministic() {
        let a = compute_authentication("pass", "salt", "challenge");
        let b = compute_authentication("pass", "salt", "challenge");
        assert_eq!(a, b);
    }

    #[test]
    fn different_passwords_give_different_auth() {
        let a = compute_authentication("password1", "salt", "challenge");
        let b = compute_authentication("password2", "salt", "challenge");
        assert_ne!(a, b);
    }
}
