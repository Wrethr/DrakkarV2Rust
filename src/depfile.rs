use std::path::{Path, PathBuf};
use crate::error::BuildError;

/// Parse a GCC-generated .d (Makefile dependency) file.
///
/// Format example:
///   target/math/utils.o: src/math/utils.cpp src/math/utils.h \
///    src/common.h
///
/// Returns a list of dependency paths (everything after the `:`)
/// including the source file itself.
pub fn parse_depfile(dep_path: &Path) -> Result<Vec<PathBuf>, BuildError> {
    let content = std::fs::read_to_string(dep_path).map_err(|e| {
        BuildError::IoError(format!("Cannot read depfile {:?}: {}", dep_path, e))
    })?;

    // Join continuation lines: replace `\\\n` (backslash + newline) with space
    let joined = join_continuation_lines(&content);

    // Find the `:` separator — everything after it is the dependency list
    let colon_pos = joined.find(':').ok_or_else(|| {
        BuildError::ParseError(format!("Depfile {:?} has no ':'", dep_path))
    })?;

    let deps_str = &joined[colon_pos + 1..];

    // Split by whitespace, filtering empty parts; unescape spaces (\ followed by space)
    let deps = split_depfile_deps(deps_str);

    Ok(deps)
}

/// Replace `\` + newline with ` ` (continuation line joining).
fn join_continuation_lines(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek() {
                Some('\n') => {
                    chars.next(); // consume \n
                    result.push(' ');
                }
                Some('\r') => {
                    chars.next(); // consume \r
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    result.push(' ');
                }
                _ => result.push(ch),
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Split dependency string by unescaped whitespace.
/// `\ ` (backslash space) is a literal space inside a path.
/// Each resulting token is a path.
fn split_depfile_deps(deps_str: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut chars = deps_str.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                match chars.peek() {
                    Some(' ') => {
                        chars.next();
                        current.push(' ');
                    }
                    Some('\\') => {
                        chars.next();
                        current.push('\\');
                    }
                    _ => {
                        // Keep the backslash (already handled continuation)
                        current.push('\\');
                    }
                }
            }
            ' ' | '\t' | '\n' | '\r' => {
                if !current.is_empty() {
                    paths.push(PathBuf::from(&current));
                    current.clear();
                }
            }
            c => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        paths.push(PathBuf::from(current));
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_continuation() {
        let s = "target/a.o: src/a.cpp \\\n src/b.h";
        let joined = join_continuation_lines(s);
        assert!(joined.contains("src/b.h"));
        assert!(!joined.contains("\\\n"));
    }

    #[test]
    fn test_split_deps() {
        let deps = split_depfile_deps(" src/a.cpp src/b.h  src/c.h ");
        assert_eq!(deps.len(), 3);
    }

    #[test]
    fn test_escaped_space_in_path() {
        let deps = split_depfile_deps(r" src/a\ b.h src/c.h");
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0], PathBuf::from("src/a b.h"));
    }
}
