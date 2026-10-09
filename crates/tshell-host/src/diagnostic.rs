//! Diagnostics preserve their locale key and arguments until the UI displays them.
use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub key: &'static str,
    pub arguments: Vec<(&'static str, String)>,
}
impl Diagnostic {
    pub fn new(key: &'static str, arguments: Vec<(&'static str, String)>) -> Self {
        Self { key, arguments }
    }
}
impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.key)?;
        for (name, value) in &self.arguments {
            write!(formatter, " {name}={value}")?;
        }
        Ok(())
    }
}
impl Error for Diagnostic {}

#[derive(Debug)]
struct DiagnosticContext {
    message: Diagnostic,
    source: anyhow::Error,
}
impl fmt::Display for DiagnosticContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}
impl Error for DiagnosticContext {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub trait ResultExt<T> {
    fn with_diagnostic(self, message: Diagnostic) -> anyhow::Result<T>;
}
impl<T, E: Into<anyhow::Error>> ResultExt<T> for Result<T, E> {
    fn with_diagnostic(self, message: Diagnostic) -> anyhow::Result<T> {
        self.map_err(|source| {
            DiagnosticContext {
                message,
                source: source.into(),
            }
            .into()
        })
    }
}
impl<T> ResultExt<T> for Option<T> {
    fn with_diagnostic(self, message: Diagnostic) -> anyhow::Result<T> {
        self.ok_or_else(|| message.into())
    }
}

pub fn find_cause<T: Error + 'static>(error: &anyhow::Error) -> Option<&T> {
    error.chain().find_map(|cause| cause.downcast_ref::<T>())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    Diagnostic(Diagnostic),
    Text(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure(pub Vec<Cause>);
impl From<&anyhow::Error> for Failure {
    fn from(error: &anyhow::Error) -> Self {
        Self(
            error
                .chain()
                .map(|cause| {
                    if let Some(context) = cause.downcast_ref::<DiagnosticContext>() {
                        Cause::Diagnostic(context.message.clone())
                    } else if let Some(diagnostic) = cause.downcast_ref::<Diagnostic>() {
                        Cause::Diagnostic(diagnostic.clone())
                    } else {
                        Cause::Text(cause.to_string())
                    }
                })
                .collect(),
        )
    }
}

#[macro_export]
macro_rules! diagnostic {
    ($key:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::Diagnostic::new($key, vec![$((stringify!($name), ($value).to_string())),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contextual_failures_keep_diagnostics_and_the_original_io_cause() {
        let error = Err::<(), _>(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "server denied /srv/archive",
        ))
        .with_diagnostic(crate::diagnostic!("sftp.op_timeout"))
        .with_diagnostic(crate::diagnostic!("sftp.op_failed"))
        .unwrap_err();
        assert_eq!(
            find_cause::<std::io::Error>(&error).unwrap().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            Failure::from(&error).0,
            vec![
                Cause::Diagnostic(crate::diagnostic!("sftp.op_failed")),
                Cause::Diagnostic(crate::diagnostic!("sftp.op_timeout")),
                Cause::Text("server denied /srv/archive".into()),
            ]
        );
    }
}
