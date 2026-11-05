// Links xkbcommon on systems that have its library but not the -dev package
// (no unversioned libxkbcommon.so): points the linker at the versioned one.
use std::path::{Path, PathBuf};

fn main() {
    let dirs = ["/usr/lib/x86_64-linux-gnu", "/usr/lib64", "/usr/lib", "/usr/lib/aarch64-linux-gnu"];
    if dirs.iter().any(|d| Path::new(d).join("libxkbcommon.so").exists()) {
        return;
    }
    let Some(found) = dirs.iter().map(|d| Path::new(d).join("libxkbcommon.so.0")).find(|p| p.exists()) else {
        return;
    };
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("link");
    std::fs::create_dir_all(&out).unwrap();
    let link = out.join("libxkbcommon.so");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(found, &link).unwrap();
    println!("cargo:rustc-link-search=native={}", out.display());
}
