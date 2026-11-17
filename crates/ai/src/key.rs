// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A remote provider's API key: held in memory, kept in the system's credential store by the host
//! (`Services::secrets`), and nowhere else. It can't be serialised (it implements neither
//! `Serialize` nor `Deserialize`), shows as `ApiKey(***)` in `Debug`, has no `Display`, and goes
//! out only in the `Authorization` header of a request to its provider.

use std::fmt;

use crate::error::AiError;

/// The longest key taken.
pub const MAX_KEY: usize = 4096;

/// A provider's API key.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// The key the owner typed or the store gave back, without the spaces around it. It must be
    /// printable ASCII without spaces (it goes into a header), at most [`MAX_KEY`] long; the error
    /// never repeats it.
    pub fn new(text: &str) -> Result<ApiKey, AiError> {
        let key = text.trim();
        if key.is_empty() {
            return Err(AiError::Config("the key is empty".into()));
        }
        if key.len() > MAX_KEY || !key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(AiError::Config("the key isn't one a provider gives (printable characters without spaces)".into()));
        }
        Ok(ApiKey(key.to_string()))
    }

    /// The key itself: for the `Authorization` header and the credential store, nothing else.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}
