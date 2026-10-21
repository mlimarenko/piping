use openssl::rand::rand_bytes;

use super::password_policy::{effective_password_min_length, validate_local_password};
use crate::errors::AppError;

const DEFAULT_GENERATED_PASSWORD_LENGTH: usize = 16;
const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &[u8] = b"0123456789";
const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

pub fn generate_local_password(configured_min_length: usize) -> Result<String, AppError> {
    let length =
        effective_password_min_length(configured_min_length).max(DEFAULT_GENERATED_PASSWORD_LENGTH);
    let mut chars = Vec::with_capacity(length);
    chars.push(random_char(LETTERS)?);
    chars.push(random_char(DIGITS)?);

    while chars.len() < length {
        chars.push(random_char(ALPHABET)?);
    }
    shuffle_chars(&mut chars)?;

    let password: String = chars.into_iter().collect();
    validate_local_password(&password, configured_min_length)?;
    Ok(password)
}

fn random_char(alphabet: &[u8]) -> Result<char, AppError> {
    let idx = random_index(alphabet.len())?;
    Ok(alphabet[idx] as char)
}

fn random_index(bound: usize) -> Result<usize, AppError> {
    if bound == 0 {
        return Err(AppError::Internal(
            "password generator received empty alphabet".to_string(),
        ));
    }
    let mut random = [0_u8; 8];
    rand_bytes(&mut random)
        .map_err(|error| AppError::Internal(format!("Failed to generate random bytes: {error}")))?;
    let value = u64::from_le_bytes(random);
    Ok((value % bound as u64) as usize)
}

fn shuffle_chars(chars: &mut [char]) -> Result<(), AppError> {
    if chars.len() < 2 {
        return Ok(());
    }
    for i in (1..chars.len()).rev() {
        let j = random_index(i + 1)?;
        chars.swap(i, j);
    }
    Ok(())
}
