use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=web/dist");
    println!("cargo:rerun-if-changed=web/dist/index.html");
    println!("cargo:rerun-if-changed=third_part/WinDivert-2.2.2-A/x64/WinDivert.dll");
    println!("cargo:rerun-if-changed=third_part/WinDivert-2.2.2-A/x64/WinDivert64.sys");

    if !Path::new("web/dist/index.html").exists() {
        println!(
            "cargo:warning=web/dist/index.html is missing. Run `pnpm --dir web install && pnpm --dir web build` before building, or use scripts/build.ps1."
        );
    }

    // WinDivert.dll and WinDivert64.sys must sit next to mitmrs.exe at runtime.
    // windivert-sys copies them into its own OUT_DIR; copy the pristine files
    // into the cargo target profile directory as well.
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&manifest).join("target"));
    let out_dir = target_dir.join(profile);
    let deps_dir = out_dir.join("deps");
    let src = PathBuf::from(&manifest).join("third_part/WinDivert-2.2.2-A/x64");
    for name in ["WinDivert.dll", "WinDivert64.sys", "WinDivert.lib"] {
        let from = src.join(name);
        for to in [out_dir.join(name), deps_dir.join(name)] {
            if from.exists() {
                let _ = std::fs::create_dir_all(to.parent().unwrap_or(&out_dir));
                let same = std::fs::metadata(&from)
                    .ok()
                    .zip(std::fs::metadata(&to).ok())
                    .map(|(a, b)| a.len() == b.len())
                    .unwrap_or(false);
                if same {
                    continue;
                }
                if let Err(e) = std::fs::copy(&from, &to) {
                    println!(
                        "cargo:warning=could not copy {name} to {}: {e}",
                        to.display()
                    );
                }
            }
        }
    }
}
