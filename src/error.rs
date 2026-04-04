use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    Network,
    Timeout,
    Auth,
    NotFound,
    Throttled,
    ServerError,
    ClientError,
    Config,
    Other,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Auth => "auth",
            Self::NotFound => "not_found",
            Self::Throttled => "throttled",
            Self::ServerError => "server_error",
            Self::ClientError => "client_error",
            Self::Config => "config",
            Self::Other => "other",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum S3Error {
    #[error("s3 ({kind}): {message}")]
    Backend { kind: ErrorKind, message: String },

    #[error("config: {0}")]
    Config(String),
}

impl S3Error {
    pub fn error_kind(&self) -> ErrorKind {
        match self {
            Self::Backend { kind, .. } => *kind,
            Self::Config(_) => ErrorKind::Config,
        }
    }

    pub(crate) fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub(crate) fn backend(kind: ErrorKind, msg: impl Into<String>) -> Self {
        Self::Backend {
            kind,
            message: msg.into(),
        }
    }
}

pub type S3Result<T> = Result<T, S3Error>;

pub(crate) fn classify<E: fmt::Debug>(err: &aws_sdk_s3::error::SdkError<E>) -> S3Error {
    use aws_sdk_s3::error::SdkError;
    match err {
        SdkError::ServiceError(e) => {
            let status = e.raw().status().as_u16();
            let kind = match status {
                401 | 403 => ErrorKind::Auth,
                404 => ErrorKind::NotFound,
                429 => ErrorKind::Throttled,
                503 if format!("{:?}", e.err()).to_lowercase().contains("slowdown") => {
                    ErrorKind::Throttled
                }
                500..=599 => ErrorKind::ServerError,
                400..=499 => ErrorKind::ClientError,
                _ => ErrorKind::Other,
            };
            S3Error::backend(kind, format!("{:?}", e.err()))
        }
        SdkError::TimeoutError(_) => S3Error::backend(ErrorKind::Timeout, err.to_string()),
        SdkError::DispatchFailure(d) if d.is_timeout() => {
            S3Error::backend(ErrorKind::Timeout, err.to_string())
        }
        SdkError::DispatchFailure(_) => S3Error::backend(ErrorKind::Network, err.to_string()),
        _ => S3Error::backend(ErrorKind::Other, err.to_string()),
    }
}
