//! Build script for LinuxVR (`lvr`).
//! Captures the latest VRChat remote config at compile time when network is available,
//! falling back to the bundled offline fallback if network is unreachable.

use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR must be set");
    let target_path = Path::new(&out_dir).join("vrchat_config_fallback.json");
    let bundled_fallback = Path::new("assets/vrchat_config_fallback.json");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/vrchat_config_fallback.json");
    println!("cargo:rerun-if-changed=c_src/dns_shield.c");

    let mut captured = false;
    let output = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "5",
            "-A",
            "lvr-builder/0.1.0",
            "https://api.vrchat.cloud/api/1/config",
        ])
        .output();

    if let Ok(out) = output
        && out.status.success()
        && let Ok(text) = std::str::from_utf8(&out.stdout)
        && text.contains("\"urlList\"")
        && serde_json::from_str::<serde_json::Value>(text).is_ok()
        && fs::write(&target_path, text).is_ok()
    {
        captured = true;
    }

    if !captured {
        let fallback_content = fs::read_to_string(bundled_fallback)
            .expect("bundled fallback config must exist in assets/");
        fs::write(&target_path, fallback_content)
            .expect("writing fallback config to OUT_DIR");
    }

    // Compile c_src/dns_shield.c into liblvr_dns_shield.so shared library
    let shim_src = Path::new("c_src/dns_shield.c");
    let shim_out = Path::new(&out_dir).join("liblvr_dns_shield.so");

    let gcc_status = Command::new("gcc")
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
        .status();

    if let Ok(st) = gcc_status && st.success() {
        // Also copy to c_src/liblvr_dns_shield.so for repository-local tests
        let _ = fs::copy(&shim_out, "c_src/liblvr_dns_shield.so");
    } else {
        panic!("Failed to compile liblvr_dns_shield.so with gcc");
    }
}
