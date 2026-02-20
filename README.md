# drakkarV2

A fast, incremental CLI build system for C/C++ projects, written in Rust using only `std`.

## Features

- **Incremental builds** via GCC-generated `.d` dependency files (`-MMD -MP -MF`)
- **Parallel compilation** — configurable worker pool (`std::sync::mpsc` + `std::thread`)
- **Mixed C/C++** — routes `.c` files through `gcc`, `.cpp/.cc/.cxx` through `g++`
- **Shell-like config parsing** — commas inside flags preserved (`-Wl,-rpath,./lib` works)
- **Mirror directory structure** — `src/math/utils.cpp` → `target/math/utils.o` (no collisions)
- **Graceful Ctrl+C** — active compiler children killed on cancellation
- **Fail-fast** (default) or `--aggregate-errors` mode
- **Zero external crates** — pure `std`

## Requirements

- Rust 1.63+ (for `available_parallelism`)
- `gcc` and/or `g++`

## Installation

```sh
git clone https://github.com/yourorg/drakkar
cd drakkar
cargo build --release
# Optionally: cp target/release/drakkar ~/.local/bin/
```

## Usage

```sh
# Create a new project
drakkar create myapp
cd myapp

# Build (debug by default)
drakkar build

# Build release
drakkar build release

# Build and run
drakkar run

# Run release build
drakkar run release

# Verbose output (prints compiler commands)
drakkar build --verbose

# Override parallel jobs
drakkar build --parallel 4

# Collect all compile errors instead of stopping at first
drakkar build --aggregate-errors

# Pass extra flags to compiler (after --)
drakkar build -- -fsanitize=address

# Show help
drakkar help
```
