use std::path::{Path, PathBuf};
use crate::error::BuildError;

#[derive(Debug, Clone, PartialEq)]
pub enum BuildProfile {
    Debug,
    Release,
}

#[derive(Debug, Clone)]
pub struct ProjectConfig {
    pub app_name: String,
    pub source_dir: PathBuf,
    pub output_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub c_flags: Vec<String>,
    pub cxx_flags: Vec<String>,
    pub ld_flags: Vec<String>,
    pub include_dirs: Vec<PathBuf>,
    pub link_libs: Vec<String>,
    pub c_standard: Option<String>,
    pub cxx_standard: Option<String>,
    pub parallel_jobs: usize,
    pub incremental: bool,
    pub preserve_temp: bool,
    pub use_process_groups: bool,
    pub gcc_path: String,
    pub gpp_path: String,
    pub verbose: bool,
    pub aggregate_errors: bool,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        ProjectConfig {
            app_name: "program".to_string(),
            source_dir: PathBuf::from("src"),
            output_dir: PathBuf::from("out"),
            temp_dir: PathBuf::from("target"),
            c_flags: vec![],
            cxx_flags: vec![],
            ld_flags: vec![],
            include_dirs: vec![],
            link_libs: vec![],
            c_standard: None,
            cxx_standard: None,
            parallel_jobs: parallelism,
            incremental: true,
            preserve_temp: true,
            use_process_groups: false,
            gcc_path: "gcc".to_string(),
            gpp_path: "g++".to_string(),
            verbose: false,
            aggregate_errors: false,
        }
    }
}

/// Shell-like tokenizer: splits a string respecting single/double quotes and backslash escaping.
/// Commas within tokens are preserved.
pub fn shell_tokenize(input: &str) -> Result<Vec<String>, BuildError> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            // Backslash escape: next char is literal
            '\\' => {
                in_token = true;
                if let Some(next) = chars.next() {
                    current.push(next);
                } else {
                    return Err(BuildError::ParseError(
                        "Trailing backslash in value".to_string(),
                    ));
                }
            }
            // Single-quoted string: everything literal until closing '
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => {
                            return Err(BuildError::ParseError(
                                "Unterminated single quote".to_string(),
                            ));
                        }
                    }
                }
            }
            // Double-quoted string: support \" and \\ inside
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => {
                            match chars.next() {
                                Some('"') => current.push('"'),
                                Some('\\') => current.push('\\'),
                                Some(' ') => current.push(' '),
                                Some('n') => current.push('\n'),
                                Some('t') => current.push('\t'),
                                Some(c) => {
                                    // Keep the backslash for unrecognized escapes
                                    current.push('\\');
                                    current.push(c);
                                }
                                None => {
                                    return Err(BuildError::ParseError(
                                        "Unterminated double quote".to_string(),
                                    ));
                                }
                            }
                        }
                        Some(c) => current.push(c),
                        None => {
                            return Err(BuildError::ParseError(
                                "Unterminated double quote".to_string(),
                            ));
                        }
                    }
                }
            }
            // Space or tab: token boundary (outside quotes)
            ' ' | '\t' => {
                if in_token {
                    tokens.push(current.clone());
                    current.clear();
                    in_token = false;
                }
            }
            // Regular character
            c => {
                in_token = true;
                current.push(c);
            }
        }
    }

    if in_token && !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

/// Parse the outer quoted value string from config line.
/// The value_str is the full RHS after `=`, e.g. `"some value"` or `"flag1 flag2"`.
/// We strip the outer quotes then tokenize the interior.
fn parse_value_str(value_str: &str, line_no: usize) -> Result<Vec<String>, BuildError> {
    let v = value_str.trim();
    // Strip optional leading/trailing outer quotes
    if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
        let inner = &v[1..v.len() - 1];
        shell_tokenize(inner).map_err(|e| {
            BuildError::ParseError(format!("Line {}: {}", line_no, e))
        })
    } else if v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2 {
        let inner = &v[1..v.len() - 1];
        shell_tokenize(inner).map_err(|e| {
            BuildError::ParseError(format!("Line {}: {}", line_no, e))
        })
    } else {
        // No outer quotes: tokenize as-is (bare value)
        shell_tokenize(v).map_err(|e| {
            BuildError::ParseError(format!("Line {}: {}", line_no, e))
        })
    }
}

fn parse_bool(s: &str, line_no: usize) -> Result<bool, BuildError> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(BuildError::ParseError(format!(
            "Line {}: expected bool (true/false), got '{}'",
            line_no, s
        ))),
    }
}

fn parse_usize(s: &str, line_no: usize) -> Result<usize, BuildError> {
    s.parse::<usize>().map_err(|_| {
        BuildError::ParseError(format!(
            "Line {}: expected integer, got '{}'",
            line_no, s
        ))
    })
}

/// Read and parse config.txt, returning a ProjectConfig.
pub fn read_config(path: &Path) -> Result<ProjectConfig, BuildError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        BuildError::ConfigError(format!("Cannot read {:?}: {}", path, e))
    })?;

    let mut cfg = ProjectConfig::default();

    for (line_idx, line) in content.lines().enumerate() {
        let line_no = line_idx + 1;
        let trimmed = line.trim();

        // Skip comments and empty lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Split on first '='
        let eq_pos = trimmed.find('=').ok_or_else(|| {
            BuildError::ParseError(format!(
                "Line {}: expected 'key = value', got '{}'",
                line_no, trimmed
            ))
        })?;

        let key = trimmed[..eq_pos].trim();
        let value_str = trimmed[eq_pos + 1..].trim();

        // Strip inline comments after the closing quote
        let value_str = strip_inline_comment(value_str);

        let tokens = parse_value_str(value_str, line_no)?;
        let first = tokens.first().map(String::as_str).unwrap_or("");

        match key {
            "app_name" => cfg.app_name = first.to_string(),
            "source_dir" => cfg.source_dir = PathBuf::from(first),
            "output_dir" => cfg.output_dir = PathBuf::from(first),
            "temp_dir" => cfg.temp_dir = PathBuf::from(first),
            "c_flags" => cfg.c_flags = tokens,
            "cxx_flags" => cfg.cxx_flags = tokens,
            "ld_flags" => cfg.ld_flags = tokens,
            "include_dirs" => {
                cfg.include_dirs = tokens.iter().map(PathBuf::from).collect();
            }
            "link_libs" => cfg.link_libs = tokens,
            "c_standard" => cfg.c_standard = if first.is_empty() { None } else { Some(first.to_string()) },
            "cxx_standard" => cfg.cxx_standard = if first.is_empty() { None } else { Some(first.to_string()) },
            "parallel_jobs" => cfg.parallel_jobs = parse_usize(first, line_no)?,
            "incremental" => cfg.incremental = parse_bool(first, line_no)?,
            "preserve_temp" => cfg.preserve_temp = parse_bool(first, line_no)?,
            "use_process_groups" => cfg.use_process_groups = parse_bool(first, line_no)?,
            "gcc_path" => cfg.gcc_path = first.to_string(),
            "gpp_path" => cfg.gpp_path = first.to_string(),
            _ => {
                // Unknown keys are silently ignored
                eprintln!(
                    "\x1b[33mwarning:\x1b[0m Line {}: unknown config key '{}'",
                    line_no, key
                );
            }
        }
    }

    Ok(cfg)
}

/// Strip trailing inline comment (anything after `"` followed by whitespace and `#`).
fn strip_inline_comment(s: &str) -> &str {
    // If the value ends with a closing quote, look for # after it
    if let Some(idx) = s.rfind('"') {
        let after = s[idx + 1..].trim();
        if after.starts_with('#') || after.is_empty() {
            return &s[..idx + 1];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_simple_flags() {
        let t = shell_tokenize("-Wall -Wextra -std=c++17").unwrap();
        assert_eq!(t, vec!["-Wall", "-Wextra", "-std=c++17"]);
    }

    #[test]
    fn test_tokenize_rpath_comma() {
        let t = shell_tokenize("-Wall -Wl,-rpath,./lib").unwrap();
        assert_eq!(t, vec!["-Wall", "-Wl,-rpath,./lib"]);
    }

    #[test]
    fn test_tokenize_quoted_spaces() {
        let t = shell_tokenize(r#"-DNAME="my name" -Wall"#).unwrap();
        assert_eq!(t, vec!["-DNAME=my name", "-Wall"]);
    }

    #[test]
    fn test_tokenize_single_quotes() {
        let t = shell_tokenize("include/ 'third party/include/'").unwrap();
        assert_eq!(t, vec!["include/", "third party/include/"]);
    }

    #[test]
    fn test_tokenize_backslash_escape() {
        let t = shell_tokenize(r"-DFOO=bar\ baz").unwrap();
        assert_eq!(t, vec!["-DFOO=bar baz"]);
    }
}
