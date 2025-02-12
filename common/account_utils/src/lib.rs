//! Provides functions that are used for key/account management across multiple crates in the
//! Lighthouse project.

use eth2_keystore::Keystore;
use eth2_wallet::{
    bip39::{Language, Mnemonic, MnemonicType},
    Wallet,
};
use filesystem::{create_with_600_perms, Error as FsError};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use std::io;
use std::io::prelude::*;
use std::path::{Path, PathBuf};
use std::str::from_utf8;
use std::thread::sleep;
use std::time::Duration;
use std::{
    fs::{self, File},
    str::FromStr,
};
use zeroize::Zeroize;

pub mod validator_definitions;

pub use eth2_keystore;
pub use eth2_wallet;
pub use eth2_wallet::PlainText;

/// The minimum number of characters required for a wallet password.
pub const MINIMUM_PASSWORD_LEN: usize = 12;
/// The `Alphanumeric` crate only generates a-z, A-Z, 0-9, therefore it has a range of 62
/// characters.
///
/// 62**48 is greater than 255**32, therefore this password has more bits of entropy than a byte
/// array of length 32.
const DEFAULT_PASSWORD_LEN: usize = 48;

pub const MNEMONIC_PROMPT: &str = "Enter the mnemonic phrase:";

pub const STDIN_INPUTS_FLAG: &str = "stdin-inputs";

/// Returns the "default" path where a wallet should store its password file.
pub fn default_wallet_password_path<P: AsRef<Path>>(wallet_name: &str, secrets_dir: P) -> PathBuf {
    secrets_dir.as_ref().join(format!("{}.pass", wallet_name))
}

/// Returns a password for a wallet, where that password is loaded from the "default" path.
pub fn default_wallet_password<P: AsRef<Path>>(
    wallet: &Wallet,
    secrets_dir: P,
) -> Result<PlainText, io::Error> {
    let path = default_wallet_password_path(wallet.name(), secrets_dir);
    fs::read(path).map(|bytes| PlainText::from(strip_off_newlines(bytes)))
}

/// Returns the "default" path where a keystore should store its password file.
pub fn default_keystore_password_path<P: AsRef<Path>>(
    keystore: &Keystore,
    secrets_dir: P,
) -> PathBuf {
    secrets_dir
        .as_ref()
        .join(format!("0x{}", keystore.pubkey()))
}

/// Reads a password file into a Zeroize-ing `PlainText` struct, with new-lines removed.
pub fn read_password<P: AsRef<Path>>(path: P) -> Result<PlainText, io::Error> {
    fs::read(path).map(strip_off_newlines).map(Into::into)
}

/// Reads a password file into a `ZeroizeString` struct, with new-lines removed.
pub fn read_password_string<P: AsRef<Path>>(path: P) -> Result<ZeroizeString, String> {
    fs::read(path)
        .map_err(|e| format!("Error opening file: {:?}", e))
        .map(strip_off_newlines)
        .and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|e| format!("Error decoding utf8: {:?}", e))
                .map(Into::into)
        })
}

/// Write a file atomically by using a temporary file as an intermediate.
///
/// Care is taken to preserve the permissions of the file at `file_path` being written.
///
/// If no file exists at `file_path` one will be created with restricted 0o600-equivalent
/// permissions.
pub fn write_file_via_temporary(
    file_path: &Path,
    temp_path: &Path,
    bytes: &[u8],
) -> Result<(), FsError> {
    // If the file already exists, preserve its permissions by copying it.
    // Otherwise, create a new file with restricted permissions.
    if file_path.exists() {
        fs::copy(file_path, temp_path).map_err(FsError::UnableToCopyFile)?;
        fs::write(temp_path, bytes).map_err(FsError::UnableToWriteFile)?;
    } else {
        create_with_600_perms(temp_path, bytes)?;
    }

    // With the temporary file created, perform an atomic rename.
    fs::rename(temp_path, file_path).map_err(FsError::UnableToRenameFile)?;

    Ok(())
}

/// Generates a random alphanumeric password of length `DEFAULT_PASSWORD_LEN` as `PlainText`.
pub fn random_password() -> PlainText {
    random_password_raw_string().into_bytes().into()
}

/// Generates a random alphanumeric password of length `DEFAULT_PASSWORD_LEN` as `ZeroizeString`.
pub fn random_password_string() -> ZeroizeString {
    random_password_raw_string().into()
}

/// Common implementation for `random_password` and `random_password_string`.
fn random_password_raw_string() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(DEFAULT_PASSWORD_LEN)
        .map(char::from)
        .collect()
}

/// Remove any number of newline or carriage returns from the end of a vector of bytes.
pub fn strip_off_newlines(mut bytes: Vec<u8>) -> Vec<u8> {
    let mut strip_off = 0;
    for (i, byte) in bytes.iter().rev().enumerate() {
        if *byte == b'\n' || *byte == b'\r' {
            strip_off = i + 1;
        } else {
            break;
        }
    }
    bytes.truncate(bytes.len() - strip_off);
    bytes
}

/// Reads a password from TTY or stdin if `use_stdin == true`.
pub fn read_password_from_user(use_stdin: bool) -> Result<ZeroizeString, String> {
    let result = if use_stdin {
        rpassword::prompt_password_stderr("")
            .map_err(|e| format!("Error reading from stdin: {}", e))
    } else {
        rpassword::read_password_from_tty(None)
            .map_err(|e| format!("Error reading from tty: {}", e))
    };

    result.map(ZeroizeString::from)
}

/// Reads a mnemonic phrase from TTY or stdin if `use_stdin == true`.
pub fn read_input_from_user(use_stdin: bool) -> Result<String, String> {
    let mut input = String::new();
    if use_stdin {
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| format!("Error reading from stdin: {}", e))?;
    } else {
        let tty = File::open("/dev/tty").map_err(|e| format!("Error opening tty: {}", e))?;
        let mut buf_reader = io::BufReader::new(tty);
        buf_reader
            .read_line(&mut input)
            .map_err(|e| format!("Error reading from tty: {}", e))?;
    }
    trim_newline(&mut input);
    Ok(input)
}

fn trim_newline(s: &mut String) {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
}

/// According to unicode, every byte that starts with 0b10xxxxxx continues encoding of character
/// Therefore the number of characters equals number of bytes minus number of 0b10xxxxxx bytes
fn count_unicode_characters(bits: &[u8]) -> usize {
    bits.iter().filter(|bit| *bit >> 6 != 2).count()
}

/// Takes a string password and checks that it meets minimum requirements.
///
/// The current minimum password requirement is a 12 character length character length.
pub fn is_password_sufficiently_complex(password: &[u8]) -> Result<(), String> {
    if count_unicode_characters(password) >= MINIMUM_PASSWORD_LEN {
        Ok(())
    } else {
        Err(format!(
            "Please use at least {} characters for your password.",
            MINIMUM_PASSWORD_LEN
        ))
    }
}

/// Returns a random 24-word english mnemonic.
pub fn random_mnemonic() -> Mnemonic {
    Mnemonic::new(MnemonicType::Words24, Language::English)
}

/// Attempts to parse a mnemonic phrase.
pub fn mnemonic_from_phrase(phrase: &str) -> Result<Mnemonic, String> {
    Mnemonic::from_phrase(phrase, Language::English).map_err(|e| e.to_string())
}

/// Provides a new-type wrapper around `String` that is zeroized on `Drop`.
///
/// Useful for ensuring that password memory is zeroed-out on drop.
#[derive(Clone, PartialEq, Serialize, Deserialize, Zeroize)]
#[zeroize(drop)]
#[serde(transparent)]
pub struct ZeroizeString(String);

impl FromStr for ZeroizeString {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

impl From<String> for ZeroizeString {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl ZeroizeString {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Remove any number of newline or carriage returns from the end of a vector of bytes.
    pub fn without_newlines(&self) -> ZeroizeString {
        let stripped_string = self.0.trim_end_matches(['\r', '\n']).into();
        Self(stripped_string)
    }
}

impl AsRef<[u8]> for ZeroizeString {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

pub fn read_mnemonic_from_cli(
    mnemonic_path: Option<PathBuf>,
    stdin_inputs: bool,
) -> Result<Mnemonic, String> {
    let mnemonic = match mnemonic_path {
        Some(path) => fs::read(&path)
            .map_err(|e| format!("Unable to read {:?}: {:?}", path, e))
            .and_then(|bytes| {
                let bytes_no_newlines: PlainText = strip_off_newlines(bytes).into();
                let phrase = from_utf8(bytes_no_newlines.as_ref())
                    .map_err(|e| format!("Unable to derive mnemonic: {:?}", e))?;
                Mnemonic::from_phrase(phrase, Language::English).map_err(|e| {
                    format!(
                        "Unable to derive mnemonic from string {:?}: {:?}",
                        phrase, e
                    )
                })
            })?,
        None => loop {
            eprintln!();
            eprintln!("{}", MNEMONIC_PROMPT);

            let mnemonic = read_input_from_user(stdin_inputs)?;

            match Mnemonic::from_phrase(mnemonic.as_str(), Language::English) {
                Ok(mnemonic_m) => {
                    eprintln!("Valid mnemonic provided.");
                    eprintln!();
                    sleep(Duration::from_secs(1));
                    break mnemonic_m;
                }
                Err(_) => {
                    eprintln!("Invalid mnemonic");
                }
            }
        },
    };
    Ok(mnemonic)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_zeroize_strip_off() {
        let expected = "hello world";

        assert_eq!(
            ZeroizeString::from("hello world\n".to_string())
                .without_newlines()
                .as_str(),
            expected
        );
        assert_eq!(
            ZeroizeString::from("hello world\n\n\n\n".to_string())
                .without_newlines()
                .as_str(),
            expected
        );
        assert_eq!(
            ZeroizeString::from("hello world\r".to_string())
                .without_newlines()
                .as_str(),
            expected
        );
        assert_eq!(
            ZeroizeString::from("hello world\r\r\r\r\r".to_string())
                .without_newlines()
                .as_str(),
            expected
        );
        assert_eq!(
            ZeroizeString::from("hello world\r\n".to_string())
                .without_newlines()
                .as_str(),
            expected
        );
        assert_eq!(
            ZeroizeString::from("hello world\r\n\r\n".to_string())
                .without_newlines()
                .as_str(),
            expected
        );
        assert_eq!(
            ZeroizeString::from("hello world".to_string())
                .without_newlines()
                .as_str(),
            expected
        );
    }

    #[test]
    fn test_strip_off() {
        let expected = b"hello world".to_vec();

        assert_eq!(strip_off_newlines(b"hello world\n".to_vec()), expected);
        assert_eq!(
            strip_off_newlines(b"hello world\n\n\n\n".to_vec()),
            expected
        );
        assert_eq!(strip_off_newlines(b"hello world\r".to_vec()), expected);
        assert_eq!(
            strip_off_newlines(b"hello world\r\r\r\r\r".to_vec()),
            expected
        );
        assert_eq!(strip_off_newlines(b"hello world\r\n".to_vec()), expected);
        assert_eq!(
            strip_off_newlines(b"hello world\r\n\r\n".to_vec()),
            expected
        );
        assert_eq!(strip_off_newlines(b"hello world".to_vec()), expected);
    }

    #[test]
    fn test_password_over_min_length() {
        is_password_sufficiently_complex(b"TestPasswordLong").unwrap();
    }

    #[test]
    fn test_password_exactly_min_length() {
        is_password_sufficiently_complex(b"TestPassword").unwrap();
    }

    #[test]
    #[should_panic]
    fn test_password_too_short() {
        is_password_sufficiently_complex(b"TestPass").unwrap();
    }

    #[test]
    fn unicode_characters() {
        assert_eq!(count_unicode_characters(b""), 0);
        assert_eq!(count_unicode_characters("🐱".to_string().as_bytes()), 1);
        assert_eq!(count_unicode_characters("🐱🐱".to_string().as_bytes()), 2);

        assert_eq!(count_unicode_characters(b"cats"), 4);
        assert_eq!(count_unicode_characters("cats🐱".to_string().as_bytes()), 5);
        assert_eq!(
            count_unicode_characters("cats🐱🐱".to_string().as_bytes()),
            6
        );
    }
}
