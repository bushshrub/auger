//! Path helpers shared by the nodes.

use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

/// Resolve `path` against `cwd` with symlinks followed as far as the path
/// exists. `None` for paths bash would expand (`~`).
pub(crate) fn resolve(cwd: &Path, path: &Path) -> Option<PathBuf> {
    if path.starts_with("~") {
        return None;
    }
    let mut lexical = PathBuf::new();
    for component in cwd.join(path).components() {
        match component {
            Component::ParentDir => {
                lexical.pop();
            }
            Component::CurDir => {}
            c => lexical.push(c),
        }
    }
    let mut existing = lexical.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            return Some(rest.iter().rev().fold(real, |p, c| p.join(c)));
        }
        rest.push(existing.file_name()?);
        existing = existing.parent()?;
    }
}

/// Whether `path`, resolved against `cwd`, lies inside `workspace`.
pub(crate) fn inside_workspace(workspace: &Path, cwd: &Path, path: &Path) -> bool {
    let Some(workspace) = resolve(workspace, Path::new("")) else {
        return false;
    };
    resolve(cwd, path).is_some_and(|p| p.starts_with(workspace))
}

const SECRET_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".kube", ".docker"];
const SECRET_FILES: &[&str] = &[".netrc", ".npmrc", ".pypirc", ".git-credentials"];

/// Whether `path` looks like it holds credentials.
pub(crate) fn is_secret(path: &Path) -> bool {
    path.components().any(|c| {
        let Component::Normal(name) = c else {
            return false;
        };
        let name = name.to_string_lossy();
        SECRET_DIRS.contains(&name.as_ref())
            || SECRET_FILES.contains(&name.as_ref())
            || name == ".env"
            || name.starts_with(".env.")
            || name.starts_with("id_rsa")
            || name.starts_with("id_ed25519")
            || name.ends_with(".pem")
            || name.ends_with(".key")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dirs_cannot_escape() {
        let ws = std::env::temp_dir().join("auger-paths-test");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        assert!(inside_workspace(&ws, &ws.join("src"), Path::new("../a.rs")));
        assert!(!inside_workspace(&ws, &ws, Path::new("../x")));
        assert!(!inside_workspace(&ws, &ws, Path::new("/etc/hosts")));
        assert!(!inside_workspace(&ws, &ws, Path::new("~/x")));
    }

    #[test]
    fn secrets() {
        assert!(is_secret(Path::new("/p/.env")));
        assert!(is_secret(Path::new("/home/u/.ssh/config")));
        assert!(!is_secret(Path::new("/p/src/env.rs")));
    }
}
