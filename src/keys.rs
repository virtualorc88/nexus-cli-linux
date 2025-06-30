//! Key management for the Nexus Network client
//!
//! Handles Ed25519 signing keys for node authentication

use ed25519_dalek::{SigningKey, VerifyingKey};
use std::fs;
use std::path::PathBuf;


pub fn get_key_path() -> Result<PathBuf, std::io::Error> {
    let home_dir = home::home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
    })?;

    fs::create_dir_all(home_dir.join(".nexus"))?;
    
    Ok(home_dir.join(".nexus").join("node_key.json"))
}

pub fn load_or_generate_signing_key() -> Result<SigningKey, Box<dyn std::error::Error>> {
    let key_path = get_key_path()?;
    
    if key_path.exists() {
        match load_signing_key(&key_path) {
            Ok(key) => return Ok(key),
            Err(_) => {
                fs::remove_file(&key_path)?;
            }
        }
    }
    
    let signing_key = SigningKey::generate(&mut rand::thread_rng());
    save_signing_key(&key_path, &signing_key)?;
    
    println!("🔑 Generated new signing key: {}", key_path.display());
    Ok(signing_key)
}


pub fn load_signing_key(path: &PathBuf) -> Result<SigningKey, Box<dyn std::error::Error>> {
    let key_data = fs::read_to_string(path)?;
    
    let key_bytes: Vec<u8> = serde_json::from_str(&key_data)?;
    
    let key_array: [u8; 32] = key_bytes.try_into()
        .map_err(|_| "Invalid key length")?;
    
    Ok(SigningKey::from_bytes(&key_array))
}


pub fn save_signing_key(path: &PathBuf, key: &SigningKey) -> Result<(), Box<dyn std::error::Error>> {
    let key_bytes = key.to_bytes().to_vec();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path.parent().unwrap())?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(path.parent().unwrap(), perms)?;
    }
    
    fs::write(path, serde_json::to_string(&key_bytes)?)?;
    
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    
    Ok(())
}


#[allow(dead_code)]
pub fn is_valid_ethereum_address(address: &str) -> bool {
    address.len() == 42 && address.starts_with("0x") && 
        address[2..].chars().all(|c| c.is_ascii_hexdigit())
}

#[allow(dead_code)]
pub fn get_public_key(key: &SigningKey) -> VerifyingKey {
    key.verifying_key()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_is_valid_ethereum_address() {
        assert!(is_valid_ethereum_address("0x1234567890abcdef1234567890abcdef12345678"));
        assert!(!is_valid_ethereum_address("0x123")); // Too short
        assert!(!is_valid_ethereum_address("1234567890abcdef1234567890abcdef12345678")); // No 0x prefix
        assert!(!is_valid_ethereum_address("0x1234567890abcdef1234567890abcdef1234567g")); // Invalid character
    }

    #[test]
    fn test_key_save_load() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("test.key");
        
        let original_key = SigningKey::generate(&mut rand::thread_rng());
        save_signing_key(&key_path, &original_key).unwrap();
        
        let loaded_key = load_signing_key(&key_path).unwrap();
        assert_eq!(original_key.to_bytes(), loaded_key.to_bytes());
    }
} 