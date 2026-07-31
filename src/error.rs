use std::fmt;

/// Error returned for invalid input, rejected biometric data, or processing failure.
#[derive(Clone, PartialEq, Eq)]
pub struct Error {
    message: String,
}

impl Error {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn processing(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn io(error: std::io::Error) -> Self {
        Self {
            message: format!("I/O error: {}", error.kind()),
        }
    }

    pub(crate) fn json(error: serde_json::Error) -> Self {
        let category = match error.classify() {
            serde_json::error::Category::Io => "I/O",
            serde_json::error::Category::Syntax => "syntax",
            serde_json::error::Category::Data => "data",
            serde_json::error::Category::Eof => "EOF",
        };
        Self {
            message: format!(
                "JSON {category} error at line {} column {}",
                error.line(),
                error.column()
            ),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Error")
            .field("message", &self.message)
            .finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::io(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::json(error)
    }
}

/// Result type used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_errors_do_not_echo_attacker_controlled_payloads() {
        let source = "\"SENTINEL_BIOMETRIC_PAYLOAD\"";
        let raw = serde_json::from_str::<crate::CaptureKind>(source).unwrap_err();
        let error = Error::json(raw);
        let rendered = error.to_string();
        assert!(rendered.contains("JSON data error"));
        assert!(!rendered.contains("SENTINEL_BIOMETRIC_PAYLOAD"));
    }
}
