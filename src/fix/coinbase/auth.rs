use base64::{Engine as _, engine::general_purpose};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoinbaseCredentials {
    pub api_key: String,
    pub passphrase: String,
    pub secret_b64: String,
}

impl CoinbaseCredentials {
    pub fn new(
        api_key: impl Into<String>,
        passphrase: impl Into<String>,
        secret_b64: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            passphrase: passphrase.into(),
            secret_b64: secret_b64.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoinbaseAuthError {
    InvalidBase64Secret,
    InvalidHmacKey,
}

impl core::fmt::Display for CoinbaseAuthError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CoinbaseAuthError {}

pub struct CoinbaseAuth;

impl CoinbaseAuth {
    /// Coinbase Exchange REST/WS prehash shape: timestamp + method + request_path + body.
    pub fn sign_rest(
        credentials: &CoinbaseCredentials,
        timestamp: &str,
        method: &str,
        request_path: &str,
        body: &str,
    ) -> Result<String, CoinbaseAuthError> {
        let prehash = format!(
            "{timestamp}{}{request_path}{body}",
            method.to_ascii_uppercase()
        );
        sign_base64_secret(&credentials.secret_b64, prehash.as_bytes())
    }

    /// Coinbase Exchange FIX Logon prehash shape documented for authenticated Logon.
    ///
    /// Official FIX connectivity sample signs the SOH-delimited string:
    /// SendingTime<SOH>MsgType<SOH>MsgSeqNum<SOH>APIKey<SOH>TargetCompID<SOH>Passphrase.
    /// The signature message has no trailing separator.
    pub fn sign_fix_logon(
        credentials: &CoinbaseCredentials,
        sending_time: &str,
        msg_type: &str,
        msg_seq_num: u64,
        target_comp_id: &str,
    ) -> Result<String, CoinbaseAuthError> {
        let prehash = format!(
            "{sending_time}{msg_type}{msg_seq_num}{}{target_comp_id}{}",
            credentials.api_key, credentials.passphrase
        );
        sign_base64_secret(&credentials.secret_b64, prehash.as_bytes())
    }
}

fn sign_base64_secret(secret_b64: &str, prehash: &[u8]) -> Result<String, CoinbaseAuthError> {
    let secret = general_purpose::STANDARD
        .decode(secret_b64)
        .map_err(|_| CoinbaseAuthError::InvalidBase64Secret)?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&secret).map_err(|_| CoinbaseAuthError::InvalidHmacKey)?;
    mac.update(prehash);
    Ok(general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}
