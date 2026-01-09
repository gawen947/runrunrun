use anyhow::{Result, anyhow};
use std::{borrow::Cow, path::PathBuf};

/// Add quotes around a string (if needed)
pub(crate) fn quote(s: &str) -> Result<Cow<'_, str>> {
    Ok(shlex::try_quote(s)?)
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

/// Parse a string as a path with tilde and environment expansion
pub(crate) fn expand(s: &str) -> Result<PathBuf> {
    // todo: add more context for anyhow, see https://docs.rs/shellexpand/latest/shellexpand/fn.full.html
    let expanded_str = shellexpand::full(s)?;
    Ok(PathBuf::from(expanded_str.as_ref()))
}
