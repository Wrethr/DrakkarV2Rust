use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum BuildError {
    IoError(String),
    ParseError(String),
    CompileError {
        src: PathBuf,
        stderr: String,
        code: Option<i32>,
    },
    LinkError {
        stderr: String,
        code: Option<i32>,
    },
    ConfigError(String),
    Cancelled,
    MultipleErrors(Vec<BuildError>),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::IoError(msg) => write!(f, "IO error: {}", msg),
            BuildError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            BuildError::CompileError { src, stderr, code } => {
                write!(f, "Compile error in {:?}", src)?;
                if let Some(c) = code {
                    write!(f, " (exit {})", c)?;
                }
                if !stderr.is_empty() {
                    write!(f, "\n{}", stderr)?;
                }
                Ok(())
            }
            BuildError::LinkError { stderr, code } => {
                write!(f, "Link error")?;
                if let Some(c) = code {
                    write!(f, " (exit {})", c)?;
                }
                if !stderr.is_empty() {
                    write!(f, "\n{}", stderr)?;
                }
                Ok(())
            }
            BuildError::ConfigError(msg) => write!(f, "Config error: {}", msg),
            BuildError::Cancelled => write!(f, "Build cancelled by user"),
            BuildError::MultipleErrors(errs) => {
                writeln!(f, "{} error(s) occurred:", errs.len())?;
                for (i, e) in errs.iter().enumerate() {
                    writeln!(f, "  [{}] {}", i + 1, e)?;
                }
                Ok(())
            }
        }
    }
}

impl From<std::io::Error> for BuildError {
    fn from(e: std::io::Error) -> Self {
        BuildError::IoError(e.to_string())
    }
}
