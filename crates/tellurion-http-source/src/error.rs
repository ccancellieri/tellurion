use crate::{PublicUrl, SourceHandle};

/// Stable, redacted categories exposed by the public source broker.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SourceErrorKind {
    Url,
    AddressDenied,
    Budget,
    Timeout,
    Redirect,
    Protocol,
    Identity,
    Invalidated,
    Range,
    Transport,
    SessionExpired,
    SourceLimit,
}

impl std::fmt::Display for SourceErrorKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Url => "invalid locator",
            Self::AddressDenied => "destination denied",
            Self::Budget => "source budget exceeded",
            Self::Timeout => "source request timed out",
            Self::Redirect => "redirect denied",
            Self::Protocol => "invalid range response",
            Self::Identity => "source identity changed",
            Self::Invalidated => "source is invalidated",
            Self::Range => "invalid byte range",
            Self::Transport => "source transport failed",
            Self::SessionExpired => "source session expired",
            Self::SourceLimit => "source session limit reached",
        })
    }
}

/// A public failure containing only a stable category and a safe subject.
#[derive(Debug, Clone)]
pub struct SourceError {
    kind: SourceErrorKind,
    subject: ErrorSubject,
}

#[derive(Debug, Clone)]
enum ErrorSubject {
    Handle(SourceHandle),
    Host {
        hostname: String,
        fingerprint: String,
    },
}

impl SourceError {
    pub fn kind(&self) -> SourceErrorKind {
        self.kind
    }

    /// Creates a redacted failure for a custom range object.
    pub fn for_handle(kind: SourceErrorKind, handle: &SourceHandle) -> Self {
        Self {
            kind,
            subject: ErrorSubject::Handle(handle.clone()),
        }
    }

    pub(crate) fn for_url(kind: SourceErrorKind, locator: &PublicUrl) -> Self {
        Self {
            kind,
            subject: ErrorSubject::Host {
                hostname: locator.host().to_owned(),
                fingerprint: locator.fingerprint().to_owned(),
            },
        }
    }
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.subject {
            ErrorSubject::Handle(handle) => write!(formatter, "{} ({handle})", self.kind),
            ErrorSubject::Host {
                hostname,
                fingerprint,
            } => write!(formatter, "{} ({hostname}; {fingerprint})", self.kind),
        }
    }
}

impl std::error::Error for SourceError {}
