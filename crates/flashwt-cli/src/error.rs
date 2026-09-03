use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Git(String),

    #[error("{0}")]
    Store(String),

    #[error("cannot {context}: {source}")]
    Io {
        context: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Usage(String),
}

impl Error {
    pub fn io(verb: &str, path: impl AsRef<Path>, source: std::io::Error) -> Self {
        let path = path.as_ref().to_path_buf();
        Error::Io {
            context: format!("{verb} {}", path.display()),
            path,
            source,
        }
    }

    pub fn io_unanchored(verb: &str, path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Error::Io {
            context: verb.to_string(),
            path: path.as_ref().to_path_buf(),
            source,
        }
    }
}

impl From<flashwt_store::Error> for Error {
    fn from(e: flashwt_store::Error) -> Self {
        Error::Store(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_display_keeps_the_historical_shape() {
        let e = Error::io("read", "/tmp/x", std::io::Error::from_raw_os_error(2));
        assert_eq!(
            e.to_string(),
            "cannot read /tmp/x: No such file or directory (os error 2)"
        );

        let e = Error::io_unanchored(
            "remove ledger",
            "/tmp/y",
            std::io::Error::from_raw_os_error(2),
        );
        assert_eq!(
            e.to_string(),
            "cannot remove ledger: No such file or directory (os error 2)"
        );
    }
}
