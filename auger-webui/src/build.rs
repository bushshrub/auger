//! Finds the webui build, building it first when it is missing or older than the sources.
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;

/// The webui build directory, or `AUGER_WEBUI_DIR` when set (used as-is).
pub fn static_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("AUGER_WEBUI_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../webui");
    let build = root.join("build");
    if !is_fresh(&root.join("src"), &build) {
        if !root.join("node_modules").exists() {
            pnpm(&root, &["install", "--frozen-lockfile"])?;
        }
        pnpm(&root, &["build"])?;
    }
    Ok(build)
}

fn is_fresh(src: &Path, build: &Path) -> bool {
    build.join("index.html").exists() && newest(build) >= newest(src)
}

/// The newest modification time under `path`.
fn newest(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    let own = meta.modified().ok();
    if !meta.is_dir() {
        return own;
    }
    let children = std::fs::read_dir(path).ok()?.flatten();
    children.map(|entry| newest(&entry.path())).fold(own, |a, b| a.max(b))
}

fn pnpm(root: &Path, args: &[&str]) -> Result<(), String> {
    eprintln!("auger-webui: pnpm {} (in {})", args.join(" "), root.display());
    let status = Command::new("pnpm")
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|error| match error.kind() {
            ErrorKind::NotFound => "pnpm is not installed; install it to build the webui".to_string(),
            _ => format!("cannot run pnpm: {error}"),
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("pnpm {} failed", args.join(" ")))
    }
}
