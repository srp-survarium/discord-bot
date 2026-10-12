// SPDX-License-Identifier: GPL-3.0-or-later
use std::{env, str::FromStr};

pub type ConfigResult<T> = Result<T, String>;

/// Environment access is isolated at the boundary and replaceable in tests.
pub struct Environment<F = fn(&str) -> Option<String>>(F);

impl Environment {
    pub fn process() -> Self {
        Self(|key| env::var(key).ok())
    }
}

impl<F: Fn(&str) -> Option<String>> Environment<F> {
    #[cfg(test)]
    pub fn new(read: F) -> Self {
        Self(read)
    }

    pub fn value(&self, key: &str) -> Option<String> {
        (self.0)(key)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    pub fn optional<T>(&self, key: &str) -> ConfigResult<Option<T>>
    where
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        self.value(key)
            .map(|raw| raw.parse().map_err(|e| format!("{key}: {e}")))
            .transpose()
    }

    pub fn required<T>(&self, key: &str) -> ConfigResult<T>
    where
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        self.optional(key)?.ok_or_else(|| format!("Set {key}"))
    }
}

/// A nonempty bot credential. Intentionally does not implement Debug or Display.
pub struct BotToken(String);
impl FromStr for BotToken {
    type Err = &'static str;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let token = raw.trim();
        if token.is_empty() || token.chars().any(char::is_whitespace) {
            return Err("expected a nonempty bot token without whitespace");
        }
        Ok(Self(token.into()))
    }
}
impl BotToken {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_errors_never_echo_input() {
        assert!("   ".parse::<BotToken>().is_err());
        assert_eq!(
            " secret with spaces ".parse::<BotToken>().err(),
            Some("expected a nonempty bot token without whitespace")
        );
        assert_eq!("  secret  ".parse::<BotToken>().unwrap().expose(), "secret");
    }
}
