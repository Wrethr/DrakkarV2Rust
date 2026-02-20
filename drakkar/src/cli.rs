use std::path::PathBuf;
use std::sync::Arc;

use crate::build::{
    collect_sources, create_project, link_objects, object_path_for, prepare_build_dirs,
};
use crate::config::{read_config, BuildProfile, ProjectConfig};
use crate::error::BuildError;
use crate::platform::register_ctrlc_handler;
use crate::worker::WorkerPool;

const HELP_TEXT: &str = r#"drakkar — C/C++ build system

USAGE:
    drakkar <command> [options]

COMMANDS:
    create <name>          Create a new project skeleton
    build [debug|release]  Build the project (default: debug)
    run   [debug|release]  Build and run the project
    help                   Show this help message

OPTIONS:
    --parallel <n>         Override number of parallel jobs
    --verbose, -v          Print compiler commands
    --aggregate-errors     Collect all compile errors instead of failing fast
    --                     Pass remaining flags to the compiler

EXAMPLES:
    drakkar create myapp
    drakkar build
    drakkar build release
    drakkar run debug
    drakkar build -- -fsanitize=address

The project must have a config.txt in the current directory.
Run `drakkar create <name>` to generate a new project with a template config.
"#;

pub struct CliArgs {
    pub command: Command,
    pub profile: BuildProfile,
    pub extra_flags: Vec<String>,
    pub parallel_override: Option<usize>,
    pub verbose: bool,
    pub aggregate_errors: bool,
}

pub enum Command {
    Create(String),
    Help,
    Build,
    Run,
}

// ─────────────────────────────────────────────
// Argument parsing
// ─────────────────────────────────────────────

pub fn parse_cli_args() -> Result<CliArgs, BuildError> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        return Ok(CliArgs {
            command: Command::Help,
            profile: BuildProfile::Debug,
            extra_flags: vec![],
            parallel_override: None,
            verbose: false,
            aggregate_errors: false,
        });
    }

    let mut command: Option<Command> = None;
    let mut profile = BuildProfile::Debug;
    let mut extra_flags: Vec<String> = Vec::new();
    let mut parallel_override: Option<usize> = None;
    let mut verbose = false;
    let mut aggregate_errors = false;
    let mut after_dashdash = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        if after_dashdash {
            extra_flags.push(arg.clone());
            i += 1;
            continue;
        }

        if arg == "--" {
            after_dashdash = true;
            i += 1;
            continue;
        }

        match arg.as_str() {
            "--verbose" | "-v" => {
                verbose = true;
            }
            "--aggregate-errors" => {
                aggregate_errors = true;
            }
            "--parallel" | "-j" => {
                i += 1;
                if i >= args.len() {
                    return Err(BuildError::ParseError(
                        "--parallel requires a number".to_string(),
                    ));
                }
                parallel_override = Some(args[i].parse::<usize>().map_err(|_| {
                    BuildError::ParseError(format!(
                        "--parallel: expected number, got '{}'",
                        args[i]
                    ))
                })?);
            }
            "help" | "--help" | "-h" => {
                command = Some(Command::Help);
            }
            "create" => {
                i += 1;
                if i >= args.len() {
                    return Err(BuildError::ParseError(
                        "'create' requires a project name".to_string(),
                    ));
                }
                command = Some(Command::Create(args[i].clone()));
            }
            "build" => {
                command = Some(Command::Build);
            }
            "run" => {
                command = Some(Command::Run);
            }
            "debug" => {
                profile = BuildProfile::Debug;
            }
            "release" => {
                profile = BuildProfile::Release;
            }
            other => {
                // Could be a flag starting with '-' (e.g. -DFOO) or unknown command
                if other.starts_with('-') {
                    extra_flags.push(other.to_string());
                } else {
                    return Err(BuildError::ParseError(format!(
                        "Unknown command or option: '{}'. Run `drakkar help`.",
                        other
                    )));
                }
            }
        }

        i += 1;
    }

    let command = command.unwrap_or(Command::Help);

    Ok(CliArgs {
        command,
        profile,
        extra_flags,
        parallel_override,
        verbose,
        aggregate_errors,
    })
}

// ─────────────────────────────────────────────
// Main run() entrypoint
// ─────────────────────────────────────────────

pub fn run() -> Result<i32, BuildError> {
    let mut cli = parse_cli_args()?;

    match &cli.command {
        Command::Help => {
            print!("{}", HELP_TEXT);
            return Ok(0);
        }
        Command::Create(name) => {
            let name = name.clone();
            create_project(&name)?;
            println!(
                "\x1b[32mProject \"{}\" created.\x1b[0m Edit {}/config.txt and add sources into {}/src/",
                name, name, name
            );
            return Ok(0);
        }
        Command::Build | Command::Run => {}
    }

    // Register Ctrl+C handler for build/run commands
    register_ctrlc_handler();

    // Read config
    let config_path = PathBuf::from("config.txt");
    if !config_path.exists() {
        return Err(BuildError::ConfigError(
            "No config.txt found in current directory. Run `drakkar create <name>` first."
                .to_string(),
        ));
    }

    let mut config = read_config(&config_path)?;

    // Apply CLI overrides
    if let Some(jobs) = cli.parallel_override {
        config.parallel_jobs = jobs;
    }
    if cli.verbose {
        config.verbose = true;
    }
    if cli.aggregate_errors {
        config.aggregate_errors = true;
    }

    let config = Arc::new(config);

    let exe_path = build_project(&config, &cli.profile, &cli.extra_flags)?;

    if let Command::Run = &cli.command {
        println!("\x1b[32mRunning\x1b[0m {:?}", exe_path);
        let status = std::process::Command::new(&exe_path)
            .status()
            .map_err(|e| BuildError::IoError(format!("Cannot run {:?}: {}", exe_path, e)))?;

        return Ok(status.code().unwrap_or(1));
    }

    Ok(0)
}

// ─────────────────────────────────────────────
// Core build pipeline
// ─────────────────────────────────────────────

pub fn build_project(
    config: &Arc<ProjectConfig>,
    profile: &BuildProfile,
    extra_flags: &[String],
) -> Result<PathBuf, BuildError> {
    let t_start = std::time::Instant::now();

    println!(
        "\x1b[1mBuilding\x1b[0m {} [{:?}]",
        config.app_name,
        profile
    );

    // Collect sources
    let source_dir = &config.source_dir;
    if !source_dir.exists() {
        return Err(BuildError::IoError(format!(
            "source_dir {:?} does not exist",
            source_dir
        )));
    }

    let sources = collect_sources(source_dir)?;

    if sources.is_empty() {
        return Err(BuildError::IoError(format!(
            "No source files found in {:?}",
            source_dir
        )));
    }

    println!("  Found {} source file(s)", sources.len());

    // Compute object paths
    let objects: Vec<_> = sources
        .iter()
        .map(|src| object_path_for(src, config))
        .collect();

    // Create directories
    prepare_build_dirs(config, &objects)?;

    // Parallel compilation
    let pool = WorkerPool::new(
        Arc::clone(config),
        profile.clone(),
        extra_flags.to_vec(),
        config.verbose,
        config.aggregate_errors,
    );

    let (compiled_objects, compiled_count) = pool.run(objects)?;

    if compiled_count == 0 {
        println!("  \x1b[32mAll up-to-date\x1b[0m — nothing to recompile.");
    } else {
        println!(
            "  \x1b[32mCompiled\x1b[0m {} file(s)",
            compiled_count
        );
    }

    // Link
    let exe_name = if cfg!(windows) {
        format!("{}.exe", config.app_name)
    } else {
        config.app_name.clone()
    };
    let out_exe = config.output_dir.join(&exe_name);

    println!("  \x1b[36mLinking\x1b[0m {}", out_exe.display());
    link_objects(
        &compiled_objects,
        &out_exe,
        config,
        profile,
        extra_flags,
        config.verbose,
    )?;

    let elapsed = t_start.elapsed();
    println!(
        "\x1b[32mFinished\x1b[0m {:?} in {:.2}s → {}",
        profile,
        elapsed.as_secs_f64(),
        out_exe.display()
    );

    Ok(out_exe)
}
