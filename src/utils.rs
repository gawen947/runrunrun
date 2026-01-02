use anyhow::{Result, anyhow};
use std::borrow::Cow;

/// Add quotes around a string (if needed)
pub(crate) fn quote(s: &str) -> Cow<'_, str> {
    shlex::quote(s)
}

/// Remove the quotes from a string, e.g. "\"hello world\"" -> "hello world"
pub(crate) fn unquote(s: &str) -> Result<String> {
    // we use shlex as it's already in the dependencies
    if let Some(parts) = shlex::split(s) {
        if parts.len() == 1 {
            return Ok(parts[0].clone());
        }
    }
    Err(anyhow!("invalid quoted string"))
}
