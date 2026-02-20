use std::path::{Path, PathBuf};
use std::time::SystemTime;
use crate::config::{ProjectConfig, BuildProfile};
use crate::error::BuildError;
use crate::depfile::parse_depfile;

#[derive(Debug, Clone, PartialEq)]
pub enum Language {
    C,
    Cpp,
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub rel_path: PathBuf,
    pub language: Language,
}

#[derive(Debug, Clone)]
pub struct ObjectFile {
    pub src: SourceFile,
    pub obj_path: PathBuf,
    pub dep_path: PathBuf,
}

// ─────────────────────────────────────────────
// Directory creation
// ─────────────────────────────────────────────

pub fn prepare_build_dirs(
    config: &ProjectConfig,
    objects: &[ObjectFile],
) -> Result<(), BuildError> {
    std::fs::create_dir_all(&config.output_dir).map_err(|e| {
        BuildError::IoError(format!(
            "Cannot create output_dir {:?}: {}",
            config.output_dir, e
        ))
    })?;
    std::fs::create_dir_all(&config.temp_dir).map_err(|e| {
        BuildError::IoError(format!(
            "Cannot create temp_dir {:?}: {}",
            config.temp_dir, e
        ))
    })?;

    for obj in objects {
        if let Some(parent) = obj.obj_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                BuildError::IoError(format!(
                    "Cannot create directory {:?}: {}",
                    parent, e
                ))
            })?;
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────
// Source collection
// ─────────────────────────────────────────────

/// Recursively collect all C/C++ source files under `source_dir`.
pub fn collect_sources(source_dir: &Path) -> Result<Vec<SourceFile>, BuildError> {
    let mut sources = Vec::new();
    collect_sources_inner(source_dir, source_dir, &mut sources)?;
    Ok(sources)
}

fn collect_sources_inner(
    root: &Path,
    dir: &Path,
    out: &mut Vec<SourceFile>,
) -> Result<(), BuildError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        BuildError::IoError(format!("Cannot read directory {:?}: {}", dir, e))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| BuildError::IoError(e.to_string()))?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        // Skip hidden directories and common build/tool dirs
        if path.is_dir() {
            if name.starts_with('.') || name == "target" || name == "out" {
                continue;
            }
            collect_sources_inner(root, &path, out)?;
        } else if path.is_file() {
            if let Some(ext) = path.extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                let language = match ext_str.as_str() {
                    "c" => Language::C,
                    "cpp" | "cc" | "cxx" | "c++" => Language::Cpp,
                    _ => continue,
                };

                let rel_path = path
                    .strip_prefix(root)
                    .map_err(|_| {
                        BuildError::IoError(format!(
                            "Cannot strip prefix {:?} from {:?}",
                            root, path
                        ))
                    })?
                    .to_path_buf();

                out.push(SourceFile {
                    path: path.clone(),
                    rel_path,
                    language,
                });
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────
// Object path computation
// ─────────────────────────────────────────────

/// Compute the object and dependency file paths for a source file.
/// Uses mirrored directory structure: temp_dir/<rel_path>.o
pub fn object_path_for(src: &SourceFile, config: &ProjectConfig) -> ObjectFile {
    let obj_path = config
        .temp_dir
        .join(src.rel_path.with_extension("o"));

    let dep_path = config
        .temp_dir
        .join(src.rel_path.with_extension("d"));

    ObjectFile {
        src: src.clone(),
        obj_path,
        dep_path,
    }
}

// ─────────────────────────────────────────────
// Incremental build check
// ─────────────────────────────────────────────

pub fn should_recompile(obj: &ObjectFile, config: &ProjectConfig) -> bool {
    // Force rebuild if incremental is disabled
    if !config.incremental {
        return true;
    }

    // Rebuild if .o doesn't exist
    let obj_meta = match std::fs::metadata(&obj.obj_path) {
        Ok(m) => m,
        Err(_) => return true,
    };

    let obj_mtime = match obj_meta.modified() {
        Ok(t) => t,
        Err(_) => return true,
    };

    // Rebuild if .d doesn't exist
    if !obj.dep_path.exists() {
        return true;
    }

    // Parse .d file to get all dependencies
    let deps = match parse_depfile(&obj.dep_path) {
        Ok(d) => d,
        Err(_) => return true, // Can't parse = rebuild
    };

    // Check if any dependency is newer than the .o
    for dep in &deps {
        if is_newer_than(dep, obj_mtime) {
            return true;
        }
    }

    false
}

fn is_newer_than(path: &Path, reference: SystemTime) -> bool {
    match std::fs::metadata(path) {
        Ok(m) => match m.modified() {
            Ok(t) => t > reference,
            Err(_) => false,
        },
        // If dep file doesn't exist (e.g., header was deleted), force rebuild
        Err(_) => true,
    }
}

// ─────────────────────────────────────────────
// Compilation
// ─────────────────────────────────────────────

/// Build the compiler argument list for a source file.
pub fn build_compile_args(
    obj: &ObjectFile,
    config: &ProjectConfig,
    profile: &BuildProfile,
    extra_flags: &[String],
) -> (String, Vec<String>) {
    let (compiler, base_flags, std_flag) = match obj.src.language {
        Language::C => (
            config.gcc_path.clone(),
            config.c_flags.clone(),
            config.c_standard.as_ref().map(|s| format!("-std={}", s)),
        ),
        Language::Cpp => (
            config.gpp_path.clone(),
            config.cxx_flags.clone(),
            config.cxx_standard.as_ref().map(|s| format!("-std={}", s)),
        ),
    };

    let mut args: Vec<String> = Vec::new();

    // Input source
    args.push("-c".to_string());
    args.push(obj.src.path.to_string_lossy().into_owned());

    // Output object
    args.push("-o".to_string());
    args.push(obj.obj_path.to_string_lossy().into_owned());

    // Base language flags
    args.extend(base_flags);

    // Standard
    if let Some(std) = std_flag {
        // Only add if not already in base_flags
        args.push(std);
    }

    // Profile-specific flags
    match profile {
        BuildProfile::Debug => {
            args.push("-g".to_string());
            args.push("-O0".to_string());
            args.push("-DDEBUG".to_string());
        }
        BuildProfile::Release => {
            args.push("-O2".to_string());
            args.push("-DNDEBUG".to_string());
        }
    }

    // Include dirs
    for inc in &config.include_dirs {
        args.push(format!("-I{}", inc.display()));
    }

    // Dependency generation
    args.push("-MMD".to_string());
    args.push("-MP".to_string());
    args.push("-MF".to_string());
    args.push(obj.dep_path.to_string_lossy().into_owned());

    // Extra CLI flags
    args.extend_from_slice(extra_flags);

    (compiler, args)
}

/// Compile a single source file to an object file.
pub fn compile_source_to_object(
    obj: &ObjectFile,
    config: &ProjectConfig,
    profile: &BuildProfile,
    extra_flags: &[String],
    verbose: bool,
    active_children: &crate::worker::ActiveChildren,
) -> Result<(), BuildError> {
    if crate::platform::is_cancelled() {
        return Err(BuildError::Cancelled);
    }

    let (compiler, args) = build_compile_args(obj, config, profile, extra_flags);

    if verbose {
        let cmd_str = format!("{} {}", compiler, args.join(" "));
        println!("  \x1b[2m$ {}\x1b[0m", cmd_str);
    }

    let mut cmd = std::process::Command::new(&compiler);
    cmd.args(&args);

    // Variant B: set process group for killpg support
    if config.use_process_groups {
        crate::platform::set_process_group(&mut cmd);
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let child = cmd.spawn().map_err(|e| {
        BuildError::IoError(format!("Failed to spawn compiler '{}': {}", compiler, e))
    })?;

    // Register child for cleanup on Ctrl+C
    let child_id = child.id();
    active_children.add(child_id);

    let output = child.wait_with_output().map_err(|e| {
        BuildError::IoError(format!("Failed to wait for compiler: {}", e))
    })?;

    active_children.remove(child_id);

    if crate::platform::is_cancelled() {
        return Err(BuildError::Cancelled);
    }

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Err(BuildError::CompileError {
            src: obj.src.path.clone(),
            stderr,
            code: output.status.code(),
        })
    }
}

// ─────────────────────────────────────────────
// Linking
// ─────────────────────────────────────────────

/// Link all object files into the final executable.
pub fn link_objects(
    objects: &[ObjectFile],
    out_exe: &PathBuf,
    config: &ProjectConfig,
    profile: &BuildProfile,
    extra_flags: &[String],
    verbose: bool,
) -> Result<(), BuildError> {
    if objects.is_empty() {
        return Err(BuildError::LinkError {
            stderr: "No object files to link".to_string(),
            code: None,
        });
    }

    let linker = &config.gpp_path;

    let mut args: Vec<String> = Vec::new();

    // Object files
    for obj in objects {
        args.push(obj.obj_path.to_string_lossy().into_owned());
    }

    // Output executable
    args.push("-o".to_string());
    let exe_path = {
        #[cfg(windows)]
        {
            let mut p = out_exe.clone();
            if p.extension().is_none() {
                p.set_extension("exe");
            }
            p
        }
        #[cfg(not(windows))]
        {
            out_exe.clone()
        }
    };
    args.push(exe_path.to_string_lossy().into_owned());

    // Linker flags
    args.extend(config.ld_flags.clone());

    // Link libraries
    args.extend(config.link_libs.clone());

    // Profile-specific
    match profile {
        BuildProfile::Release => {
            args.push("-s".to_string()); // strip symbols
        }
        BuildProfile::Debug => {}
    }

    // Extra CLI flags
    args.extend_from_slice(extra_flags);

    if verbose {
        println!("  \x1b[2m$ {} {}\x1b[0m", linker, args.join(" "));
    }

    let mut cmd = std::process::Command::new(linker);
    cmd.args(&args);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = cmd.spawn().map_err(|e| {
        BuildError::IoError(format!("Failed to spawn linker '{}': {}", linker, e))
    })?.wait_with_output().map_err(|e| {
        BuildError::IoError(format!("Failed to wait for linker: {}", e))
    })?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Err(BuildError::LinkError {
            stderr,
            code: output.status.code(),
        })
    }
}

// ─────────────────────────────────────────────
// Project creation skeleton
// ─────────────────────────────────────────────

pub fn create_project(name: &str) -> Result<(), BuildError> {
    let root = PathBuf::from(name);

    if root.exists() {
        return Err(BuildError::IoError(format!(
            "Directory '{}' already exists",
            name
        )));
    }

    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(root.join("out"))?;
    std::fs::create_dir_all(root.join("target"))?;

    let config_content = format!(
        r#"# drakkar config — project: {name}
app_name = "{name}"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"

# Compiler flags
c_flags = "-Wall -Wextra -std=c11"
cxx_flags = "-Wall -Wextra -std=c++17"
ld_flags = ""
include_dirs = ""
link_libs = ""

# Standards
c_standard = "c11"
cxx_standard = "c++17"

# Compiler paths (defaults: gcc, g++)
gcc_path = "gcc"
gpp_path = "g++"

# Build options
parallel_jobs = "4"
incremental = "true"
preserve_temp = "true"
use_process_groups = "false"
"#,
        name = name
    );

    std::fs::write(root.join("config.txt"), config_content)?;

    let readme_content = format!(
        r#"# {name}

A C/C++ project built with [drakkar](https://github.com/yourorg/drakkar).

## Building

```sh
drakkar build           # debug build
drakkar build release   # release build
drakkar run             # build & run
```

## Project structure

```
src/        — source files (.c, .cpp, .cc, .cxx)
out/        — compiled binaries
target/     — object files and dependency files (.o, .d)
config.txt  — build configuration
```
"#,
        name = name
    );
    std::fs::write(root.join("README.md"), readme_content)?;

    // Write a sample main.cpp
    let main_content = r#"#include <iostream>

int main() {
    std::cout << "Hello from drakkar!" << std::endl;
    return 0;
}
"#;
    std::fs::write(root.join("src").join("main.cpp"), main_content)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_collect_sources_skips_hidden() {
        let dir = std::env::temp_dir().join("drakkar_test_collect");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/main.cpp"), "").unwrap();
        fs::write(dir.join(".git/config"), "").unwrap();

        let sources = collect_sources(&dir.join("src")).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].rel_path, PathBuf::from("main.cpp"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_object_path_for_mirror() {
        use crate::config::ProjectConfig;
        let mut cfg = ProjectConfig::default();
        cfg.temp_dir = PathBuf::from("target");

        let src = SourceFile {
            path: PathBuf::from("src/math/utils.cpp"),
            rel_path: PathBuf::from("math/utils.cpp"),
            language: Language::Cpp,
        };

        let obj = object_path_for(&src, &cfg);
        assert_eq!(obj.obj_path, PathBuf::from("target/math/utils.o"));
        assert_eq!(obj.dep_path, PathBuf::from("target/math/utils.d"));
    }

    #[test]
    fn test_no_name_collision() {
        use crate::config::ProjectConfig;
        let cfg = ProjectConfig::default();

        let src1 = SourceFile {
            path: PathBuf::from("src/math/utils.cpp"),
            rel_path: PathBuf::from("math/utils.cpp"),
            language: Language::Cpp,
        };
        let src2 = SourceFile {
            path: PathBuf::from("src/network/utils.cpp"),
            rel_path: PathBuf::from("network/utils.cpp"),
            language: Language::Cpp,
        };

        let obj1 = object_path_for(&src1, &cfg);
        let obj2 = object_path_for(&src2, &cfg);
        assert_ne!(obj1.obj_path, obj2.obj_path);
    }
}
