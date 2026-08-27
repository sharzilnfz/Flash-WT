//! Typed errors at the crate edge (arch-hardening ticket 03 item 4),
//! replacing the former `Result<_, String>` plumbing.
//!
//! Display strings double as the user-facing messages printed after
//! `wt: ` on stderr; every variant renders exactly the wording its
//! string predecessor produced, because end-to-end suites parse that
//! prose.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A git plumbing failure (spawn failure, nonzero exit, missing
    /// repository); carries a ready-to-print message.
    #[error("{0}")]
    Git(String),

    /// The store or a hydration step refused; carries a ready-to-print
    /// message, usually ending in the underlying error's Display.
    #[error("{0}")]
    Store(String),

    /// A filesystem failure anchored at one path. `context` is the
    /// pre-rendered operation ("read /a/b") so Display reproduces the
    /// historical `cannot <verb> <path>: <source>` shape while the
    /// structured fields stay available to callers.
    #[error("cannot {context}: {source}")]
    Io {
        context: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Misuse the user must fix: bad flags, bad env values. Exits 2,
    /// matching clap's own parse-error convention.
    #[error("{0}")]
    Usage(String),
}

impl Error {
    /// Anchor an io failure at `path`; the rendered message is
    /// `cannot <verb> <path>: <source>`.
    pub fn io(verb: &str, path: impl AsRef<Path>, source: std::io::Error) -> Self {
        let path = path.as_ref().to_path_buf();
        Error::Io {
            context: format!("{verb} {}", path.display()),
            path,
            source,
        }
    }

    /// An io failure whose message deliberately omits the path (the
    /// historical wording for a few fixed names like the hydration
    /// ledger); `path` is still recorded structurally.
    pub fn io_unanchored(verb: &str, path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Error::Io {
            context: verb.to_string(),
            path: path.as_ref().to_path_buf(),
            source,
        }
    }
}

impl From<wt_store::Error> for Error {
    fn from(e: wt_store::Error) -> Self {
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
