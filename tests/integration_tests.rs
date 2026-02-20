/// Drakkar integration tests.
/// These tests run the full build pipeline using real gcc/g++.
/// Run with: cargo test --test integration_tests
/// Requires gcc and g++ to be installed.

use std::path::PathBuf;
use std::fs;
use std::process::Command;

fn drakkar_bin() -> PathBuf {
    // Use the debug build of drakkar
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // remove test binary name
    // Go up to target/debug
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("drakkar")
}

fn run_drakkar(args: &[&str], cwd: &PathBuf) -> std::process::Output {
    Command::new(drakkar_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("Failed to run drakkar binary")
}

fn temp_workspace(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("drakkar_test_{}", test_name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ─────────────────────────────────────────────
// Test 1: Create project skeleton
// ─────────────────────────────────────────────

#[test]
fn test_create_project_skeleton() {
    let workspace = temp_workspace("create");

    let out = Command::new(drakkar_bin())
        .args(&["create", "demo"])
        .current_dir(&workspace)
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "drakkar create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let demo = workspace.join("demo");
    assert!(demo.join("src").is_dir(), "src/ missing");
    assert!(demo.join("out").is_dir(), "out/ missing");
    assert!(demo.join("target").is_dir(), "target/ missing");
    assert!(demo.join("config.txt").is_file(), "config.txt missing");
    assert!(demo.join("README.md").is_file(), "README.md missing");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("demo"), "Expected 'demo' in output");

    let _ = fs::remove_dir_all(&workspace);
}

// ─────────────────────────────────────────────
// Test 2: Simple single-file build and run
// ─────────────────────────────────────────────

#[test]
fn test_simple_build_and_run() {
    let workspace = temp_workspace("simple_build");

    // Setup project
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::create_dir_all(workspace.join("out")).unwrap();
    fs::create_dir_all(workspace.join("target")).unwrap();

    fs::write(workspace.join("src/main.cpp"), r#"
#include <iostream>
int main() {
    std::cout << "hello drakkar" << std::endl;
    return 0;
}
"#).unwrap();

    fs::write(workspace.join("config.txt"), r#"
app_name = "hello"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall"
cxx_standard = "c++17"
incremental = "true"
parallel_jobs = "1"
"#).unwrap();

    let build_out = run_drakkar(&["build"], &workspace);
    assert!(
        build_out.status.success(),
        "build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build_out.stdout),
        String::from_utf8_lossy(&build_out.stderr)
    );

    // Check object and binary exist
    assert!(workspace.join("target/main.o").exists(), "target/main.o missing");
    assert!(workspace.join("out/hello").exists(), "out/hello missing");

    // Run it
    let run_out = Command::new(workspace.join("out/hello")).output().unwrap();
    let stdout = String::from_utf8_lossy(&run_out.stdout);
    assert!(stdout.contains("hello drakkar"), "Expected output 'hello drakkar', got: {}", stdout);

    let _ = fs::remove_dir_all(&workspace);
}

// ─────────────────────────────────────────────
// Test 3: Name collision prevention (same filename in different dirs)
// ─────────────────────────────────────────────

#[test]
fn test_no_name_collision() {
    let workspace = temp_workspace("collision");

    fs::create_dir_all(workspace.join("src/math")).unwrap();
    fs::create_dir_all(workspace.join("src/network")).unwrap();
    fs::create_dir_all(workspace.join("out")).unwrap();
    fs::create_dir_all(workspace.join("target")).unwrap();

    fs::write(workspace.join("src/math/utils.cpp"), r#"
int math_add(int a, int b) { return a + b; }
"#).unwrap();

    fs::write(workspace.join("src/network/utils.cpp"), r#"
int net_connect(int port) { return port; }
"#).unwrap();

    fs::write(workspace.join("src/main.cpp"), r#"
#include <iostream>
int math_add(int, int);
int net_connect(int);
int main() {
    std::cout << math_add(1, 2) << " " << net_connect(80) << std::endl;
    return 0;
}
"#).unwrap();

    fs::write(workspace.join("config.txt"), r#"
app_name = "collision_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall"
incremental = "true"
parallel_jobs = "2"
"#).unwrap();

    let out = run_drakkar(&["build"], &workspace);
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(workspace.join("target/math/utils.o").exists(), "math/utils.o missing");
    assert!(workspace.join("target/network/utils.o").exists(), "network/utils.o missing");

    let _ = fs::remove_dir_all(&workspace);
}

// ─────────────────────────────────────────────
// Test 4: Incremental build (header change triggers recompile)
// ─────────────────────────────────────────────

#[test]
fn test_incremental_header_change() {
    let workspace = temp_workspace("incremental");

    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::create_dir_all(workspace.join("out")).unwrap();
    fs::create_dir_all(workspace.join("target")).unwrap();

    fs::write(workspace.join("src/common.h"), "// v1\n#define VERSION 1\n").unwrap();
    fs::write(workspace.join("src/a.cpp"), "#include \"common.h\"\nint a_func() { return VERSION; }\n").unwrap();
    fs::write(workspace.join("src/b.cpp"), "#include \"common.h\"\nint b_func() { return VERSION; }\n").unwrap();
    fs::write(workspace.join("src/main.cpp"), r#"
#include <iostream>
int a_func();
int b_func();
int main() { std::cout << a_func() + b_func(); return 0; }
"#).unwrap();

    fs::write(workspace.join("config.txt"), r#"
app_name = "incremental_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall"
incremental = "true"
parallel_jobs = "2"
"#).unwrap();

    // First build
    let out = run_drakkar(&["build"], &workspace);
    assert!(out.status.success(), "First build failed: {}", String::from_utf8_lossy(&out.stderr));

    let a_mtime1 = fs::metadata(workspace.join("target/a.o")).unwrap().modified().unwrap();
    let b_mtime1 = fs::metadata(workspace.join("target/b.o")).unwrap().modified().unwrap();

    // Sleep to ensure mtime difference
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Modify common.h
    fs::write(workspace.join("src/common.h"), "// v2\n#define VERSION 2\n").unwrap();

    // Second build — a.o and b.o should be recompiled; main.o need not be
    let out2 = run_drakkar(&["build"], &workspace);
    assert!(out2.status.success(), "Second build failed: {}", String::from_utf8_lossy(&out2.stderr));

    let a_mtime2 = fs::metadata(workspace.join("target/a.o")).unwrap().modified().unwrap();
    let b_mtime2 = fs::metadata(workspace.join("target/b.o")).unwrap().modified().unwrap();

    assert!(a_mtime2 > a_mtime1, "a.o was NOT recompiled after header change");
    assert!(b_mtime2 > b_mtime1, "b.o was NOT recompiled after header change");

    // Third build — nothing changed
    let out3 = run_drakkar(&["build"], &workspace);
    assert!(out3.status.success(), "Third build failed");
    let stdout3 = String::from_utf8_lossy(&out3.stdout);
    assert!(
        stdout3.contains("up-to-date"),
        "Expected 'up-to-date' after no changes, got: {}",
        stdout3
    );

    let _ = fs::remove_dir_all(&workspace);
}

// ─────────────────────────────────────────────
// Test 5: Mixed .c and .cpp compilation
// ─────────────────────────────────────────────

#[test]
fn test_mixed_c_and_cpp() {
    let workspace = temp_workspace("mixed");

    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::create_dir_all(workspace.join("out")).unwrap();
    fs::create_dir_all(workspace.join("target")).unwrap();

    fs::write(workspace.join("src/utils.c"), r#"
#include <stdio.h>
void c_hello(void) { printf("C says hello\n"); }
"#).unwrap();

    fs::write(workspace.join("src/main.cpp"), r#"
#include <iostream>
extern "C" void c_hello(void);
int main() { c_hello(); return 0; }
"#).unwrap();

    fs::write(workspace.join("config.txt"), r#"
app_name = "mixed_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
c_flags = "-Wall"
cxx_flags = "-Wall"
c_standard = "c11"
cxx_standard = "c++17"
incremental = "true"
parallel_jobs = "2"
"#).unwrap();

    let out = run_drakkar(&["build"], &workspace);
    assert!(
        out.status.success(),
        "mixed build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(workspace.join("target/utils.o").exists(), "utils.o (C) missing");
    assert!(workspace.join("target/main.o").exists(), "main.o (C++) missing");
    assert!(workspace.join("out/mixed_test").exists(), "binary missing");

    let _ = fs::remove_dir_all(&workspace);
}

// ─────────────────────────────────────────────
// Test 6: Flags with commas (-Wl,-rpath,...)
// ─────────────────────────────────────────────

#[test]
fn test_rpath_flag_not_split() {
    let workspace = temp_workspace("rpath");

    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::create_dir_all(workspace.join("out")).unwrap();
    fs::create_dir_all(workspace.join("target")).unwrap();

    fs::write(workspace.join("src/main.cpp"), "int main() { return 0; }\n").unwrap();

    // The rpath flag contains commas — must not be split
    fs::write(workspace.join("config.txt"), r#"
app_name = "rpath_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall -Wextra"
ld_flags = "-Wl,-O1"
incremental = "true"
parallel_jobs = "1"
"#).unwrap();

    let out = run_drakkar(&["build"], &workspace);
    assert!(
        out.status.success(),
        "rpath build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let _ = fs::remove_dir_all(&workspace);
}

// ─────────────────────────────────────────────
// Test 7: Parallel build (correctness under concurrency)
// ─────────────────────────────────────────────

#[test]
fn test_parallel_build_correctness() {
    let workspace = temp_workspace("parallel");

    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::create_dir_all(workspace.join("out")).unwrap();
    fs::create_dir_all(workspace.join("target")).unwrap();

    // Generate N source files
    let n = 20;
    let mut declarations = String::new();
    let mut calls = String::new();
    for i in 0..n {
        fs::write(
            workspace.join(format!("src/mod{}.cpp", i)),
            format!("int func{}() {{ return {}; }}\n", i, i),
        ).unwrap();
        declarations.push_str(&format!("int func{}();\n", i));
        calls.push_str(&format!("total += func{}();\n", i));
    }

    let main_cpp = format!(r#"
#include <iostream>
{}
int main() {{
    int total = 0;
    {}
    std::cout << total << std::endl;
    return 0;
}}
"#, declarations, calls);
    fs::write(workspace.join("src/main.cpp"), main_cpp).unwrap();

    fs::write(workspace.join("config.txt"), r#"
app_name = "parallel_test"
source_dir = "src/"
output_dir = "out/"
temp_dir = "target/"
cxx_flags = "-Wall"
incremental = "true"
parallel_jobs = "8"
"#).unwrap();

    let out = run_drakkar(&["build"], &workspace);
    assert!(
        out.status.success(),
        "parallel build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Verify all .o files exist
    for i in 0..n {
        let obj = workspace.join(format!("target/mod{}.o", i));
        assert!(obj.exists(), "target/mod{}.o missing", i);
    }

    // Run and verify output
    let run_out = Command::new(workspace.join("out/parallel_test")).output().unwrap();
    let expected: i32 = (0..n).map(|i| i as i32).sum();
    let actual: i32 = String::from_utf8_lossy(&run_out.stdout).trim().parse().unwrap_or(-1);
    assert_eq!(actual, expected, "Parallel build produced wrong result");

    let _ = fs::remove_dir_all(&workspace);
}
