//! Build script for LinuxVR (`lvr`).

use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR must be set");

    register_rerun_triggers();
    check_no_legacy_code();
    check_file_lengths();
    fetch_vrchat_config(&out_dir);
    compile_dns_shield(&out_dir);
}

/// Tells Cargo which files should trigger a rebuild.
fn register_rerun_triggers() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/vrchat_config_fallback.json");
    println!("cargo:rerun-if-changed=c_src/dns_shield.c");
    println!("cargo:rerun-if-changed=src");
}

/// Fetches the latest VRChat remote config at compile time when network is
/// available, falling back to the bundled offline fallback otherwise.
fn fetch_vrchat_config(out_dir: &str) {
    let target_path = Path::new(out_dir).join("vrchat_config_fallback.json");
    let bundled_fallback = Path::new("assets/vrchat_config_fallback.json");

    let fetched = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "5",
            "-A",
            "lvr-builder/0.1.0",
            "https://api.vrchat.cloud/api/1/config",
        ])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .filter(|text| text.contains("\"urlList\""))
        .filter(|text| serde_json::from_str::<serde_json::Value>(text).is_ok())
        .and_then(|text| fs::write(&target_path, &text).ok().map(|_| text));

    if fetched.is_none() {
        let fallback = fs::read_to_string(bundled_fallback)
            .expect("bundled fallback config must exist in assets/");
        fs::write(&target_path, fallback).expect("writing fallback config to OUT_DIR");
    }
}

/// Compiles `c_src/dns_shield.c` into `liblvr_dns_shield.so` and embeds it.
fn compile_dns_shield(out_dir: &str) {
    let shim_src = Path::new("c_src/dns_shield.c");
    let shim_out = Path::new(out_dir).join("liblvr_dns_shield.so");

    let ok = Command::new("gcc")
        .args([
            "-shared",
            "-fPIC",
            "-O3",
            "-Wall",
            "-Wextra",
            shim_src.to_str().unwrap(),
            "-o",
            shim_out.to_str().unwrap(),
            "-ldl",
            "-lpthread",
        ])
        .status()
        .map(|st| st.success())
        .unwrap_or(false);

    if !ok {
        panic!("Failed to compile liblvr_dns_shield.so with gcc");
    }

    // Also copy to c_src/ for repository-local tests
    let _ = fs::copy(&shim_out, "c_src/liblvr_dns_shield.so");
}

/// Fails the build if any `.rs` file in `src/` exceeds 1000 lines.
fn check_file_lengths() {
    const MAX_LINES: usize = 1000;
    let src_dir = Path::new("src");
    let mut violations: Vec<String> = Vec::new();

    visit_rs_files(src_dir, &mut |path, content| {
        let count = content.lines().count();
        if count > MAX_LINES {
            violations.push(format!(
                "{}: {} lines (max {MAX_LINES})",
                path.display(),
                count,
            ));
        }
    });

    if !violations.is_empty() {
        eprintln!();
        eprintln!("=== Source files exceed {MAX_LINES}-line limit ===");
        eprintln!();
        for v in &violations {
            eprintln!("  {v}");
        }
        eprintln!();
        panic!(
            "{} file(s) exceed the {MAX_LINES}-line limit. See output above.",
            violations.len()
        );
    }
}

/// Scans all `.rs` files under `src/` and fails the build if any line
/// contains a banned legacy/compatibility keyword in a comment or string.
///
/// Banned (case-insensitive substring match):
///   legacy, compatibility, compat, backward, migration, migrate, migrat, deprecated
fn check_no_legacy_code() {
    const BANNED: &[&str] = &[
        "legacy",
        "compatibility",
        "compat",
        "backward",
        "migration",
        "migrate",
        "migrat",
        "deprecated",
    ];

    // Substrings that are third-party names we don't control and are always
    // safe to ignore (e.g. Steam filesystem paths, VDF key names).
    const ALLOWED: &[&str] = &[
        "compatdata",        // Steam Proton prefix path component
        "CompatToolMapping", // Steam VDF key name
        "compat_tool",       // Steam test function names referencing CompatToolMapping
    ];

    let src_dir = Path::new("src");
    let mut violations: Vec<String> = Vec::new();

    visit_rs_files(src_dir, &mut |path, content| {
        for (line_no, line) in content.lines().enumerate() {
            let lower = line.to_lowercase();
            // Skip lines covered by an allowed exception
            if ALLOWED.iter().any(|&ok| lower.contains(&ok.to_lowercase())) {
                continue;
            }
            for &banned in BANNED {
                if lower.contains(banned) {
                    violations.push(format!(
                        "{}:{}: banned keyword {:?} — {}",
                        path.display(),
                        line_no + 1,
                        banned,
                        line.trim(),
                    ));
                    break; // one report per line is enough
                }
            }
        }
    });

    if !violations.is_empty() {
        eprintln!();
        eprintln!("=== Legacy / compatibility code detected — REMOVE before shipping ===");
        eprintln!();
        for v in &violations {
            eprintln!("  {v}");
        }
        eprintln!();
        panic!(
            "{} legacy/compatibility violation(s) found in src/. See output above.",
            violations.len()
        );
    }
}

/// Recursively visits every `.rs` file under `dir`, calling `cb` with the
/// path and file content.
fn visit_rs_files(dir: &Path, cb: &mut impl FnMut(&Path, &str)) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            visit_rs_files(&path, cb);
        } else if path.extension().is_some_and(|e| e == "rs") {
            if let Ok(text) = fs::read_to_string(&path) {
                cb(&path, &text);
            }
        }
    }
}
