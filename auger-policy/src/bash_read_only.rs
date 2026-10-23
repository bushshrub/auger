//! Allows bash commands that only read inside the workspace.

use crate::paths::inside_workspace;
use crate::paths::is_secret;
use crate::paths::resolve;
use auger_core::PolicyCall;
use auger_core::PolicyNode;
use auger_core::Verdict;
use auger_shell::Arg;
use auger_shell::Command;
use auger_shell::Joint;
use auger_shell::Redirect;
use auger_shell::plain_commands;
use std::path::Path;
use std::path::PathBuf;

/// Allows plain commands built only from read-only programs, where every
/// path argument stays inside the workspace and is not a secret. Anything it
/// cannot classify passes to the next node.
///
/// Known gap: a short flag with an attached value (`-f/etc/x`) is not checked
/// as a path. No allowlisted program reads a file through one.
pub struct BashReadOnly;

impl PolicyNode for BashReadOnly {
    fn check(&self, call: &PolicyCall<'_>) -> Verdict {
        let Some(commands) = call.command.and_then(plain_commands) else {
            return Verdict::Next;
        };
        if read_only(&commands, call.cwd, call.workspace) {
            Verdict::Allow {
                node: "bash-read-only",
            }
        } else {
            Verdict::Next
        }
    }
}

/// How a program's arguments are checked.
enum Rule {
    /// Arguments are never read as files.
    Free,
    /// Every non-flag argument must be a readable path.
    Paths,
    /// Like `Paths`, but no expanding words and none of these flags (an
    /// argument equal to one, or starting with it followed by `=`).
    Banned(&'static [&'static str]),
}

const FIND_BANNED: &[&str] = &[
    "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fls", "-fprint", "-fprint0", "-fprintf",
];
const RG_BANNED: &[&str] = &["--pre", "--pre-glob", "--hostname-bin"];
const SORT_BANNED: &[&str] = &["-o", "--output", "--compress-program"];
const GIT_BANNED: &[&str] = &["--output", "--ext-diff", "--textconv"];
const GIT_SUBCOMMANDS: &[&str] = &["status", "diff", "log", "show", "rev-parse", "ls-files", "blame"];

fn rule(program: &str) -> Option<Rule> {
    Some(match program {
        "echo" | "printf" | "true" | "pwd" => Rule::Free,
        "cat" | "head" | "tail" | "wc" | "ls" | "grep" | "egrep" | "fgrep" | "file" | "stat"
        | "which" | "basename" | "dirname" | "realpath" | "tree" | "du" | "diff" | "cmp"
        | "jq" | "nl" | "cut" | "tr" => Rule::Paths,
        "find" => Rule::Banned(FIND_BANNED),
        "rg" => Rule::Banned(RG_BANNED),
        "sort" => Rule::Banned(SORT_BANNED),
        _ => return None,
    })
}

fn read_only(commands: &[Command], cwd: &Path, workspace: &Path) -> bool {
    if !inside_workspace(workspace, cwd, Path::new("")) {
        return false;
    }
    let mut cwd = cwd.to_path_buf();
    for (i, command) in commands.iter().enumerate() {
        if command.argv[0].text == "cd" {
            match leading_cd(commands, i, &cwd, workspace) {
                Some(dir) => cwd = dir,
                None => return false,
            }
            continue;
        }
        let ok = command
            .redirects
            .iter()
            .all(|r| redirect_ok(r, &cwd, workspace))
            && command_ok(&command.argv, &cwd, workspace);
        if !ok {
            return false;
        }
    }
    true
}

/// `cd <dir> && ...` as the first command, with no `;` or `||` later that
/// could run something in the old directory if the `cd` fails.
fn leading_cd(commands: &[Command], i: usize, cwd: &Path, workspace: &Path) -> Option<PathBuf> {
    let [_, dir] = commands[i].argv.as_slice() else {
        return None;
    };
    let only_and = commands
        .iter()
        .skip(1)
        .all(|c| matches!(c.joint, Some(Joint::And | Joint::Pipe)));
    let followed_by_and = commands.get(1).is_none_or(|c| c.joint == Some(Joint::And));
    if i != 0 || dir.expands || !only_and || !followed_by_and || !commands[0].redirects.is_empty() {
        return None;
    }
    let target = resolve(cwd, Path::new(&dir.text))?;
    inside_workspace(workspace, cwd, &target).then_some(target)
}

fn redirect_ok(redirect: &Redirect, cwd: &Path, workspace: &Path) -> bool {
    let target = &redirect.target.text;
    match redirect.op.as_str() {
        ">&" | "<&" => target == "-" || target.chars().all(|c| c.is_ascii_digit()),
        ">" | ">>" | "&>" | "&>>" | ">|" => target == "/dev/null",
        "<" => path_ok(&redirect.target, cwd, workspace),
        _ => false,
    }
}

fn command_ok(argv: &[Arg], cwd: &Path, workspace: &Path) -> bool {
    let (program, args) = (&argv[0], &argv[1..]);
    if program.expands {
        return false;
    }
    if program.text == "git" {
        return git_ok(args, cwd, workspace);
    }
    match rule(&program.text) {
        Some(Rule::Free) => true,
        Some(Rule::Paths) => args.iter().all(|a| path_ok(a, cwd, workspace)),
        Some(Rule::Banned(banned)) => {
            // `sort -ro out` hides `-o` in a cluster of short flags.
            let clustered_o = program.text == "sort"
                && args
                    .iter()
                    .any(|a| a.text.starts_with('-') && !a.text.starts_with("--") && a.text.contains('o'));
            !clustered_o
                && args.iter().all(|a| {
                    !a.expands && !is_banned(&a.text, banned) && path_ok(a, cwd, workspace)
                })
        }
        None => false,
    }
}

/// `git <read-only subcommand> ...`. Global options before the subcommand
/// (`-c`, `-C`, `--git-dir`) can run arbitrary programs, so none are allowed.
fn git_ok(args: &[Arg], cwd: &Path, workspace: &Path) -> bool {
    let Some((subcommand, rest)) = args.split_first() else {
        return false;
    };
    GIT_SUBCOMMANDS.contains(&subcommand.text.as_str())
        && rest.iter().all(|a| {
            !a.expands && !is_banned(&a.text, GIT_BANNED) && path_ok(a, cwd, workspace)
        })
}

fn is_banned(arg: &str, banned: &[&str]) -> bool {
    banned.iter().any(|flag| {
        arg.strip_prefix(flag)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
    })
}

/// Flags pass, except that the value of `--flag=value` is checked like any
/// other argument. Everything else must resolve inside the workspace and not
/// look like a secret. Patterns (`grep foo`) are checked too: harmless when
/// relative, and an absolute pattern only costs a prompt.
fn path_ok(arg: &Arg, cwd: &Path, workspace: &Path) -> bool {
    let text = match arg.text.strip_prefix("--") {
        Some(long) => match long.split_once('=') {
            Some((_, value)) => value,
            None => return true,
        },
        None if arg.text.starts_with('-') => return true,
        None => &arg.text,
    };
    if arg.expands && text.starts_with('~') {
        return false;
    }
    let path = Path::new(text);
    inside_workspace(workspace, cwd, path)
        && !is_secret(path)
        && resolve(cwd, path).is_some_and(|p| !is_secret(&p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(cmd: &str) -> bool {
        let ws = std::env::temp_dir().join("auger-bash-ro-test");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        let commands = plain_commands(&cmd.replace("$WS", ws.to_str().unwrap()))
            .unwrap_or_else(|| panic!("unparsed: {cmd}"));
        read_only(&commands, &ws, &ws)
    }

    #[test]
    fn allows_reads_inside_workspace() {
        for cmd in [
            "cd $WS && cat src/a.rs && echo '=== x ===' && cat src/b.rs",
            r#"grep -rn "todo!\|unimplemented!" --include=*.rs --exclude-dir=target . | head -20"#,
            "git diff --stat 2>&1 | tail -40",
            "find . -name '*.rs' | wc -l",
            "ls -la src >/dev/null",
        ] {
            assert!(allowed(cmd), "denied: {cmd}");
        }
    }

    #[test]
    fn passes_on_anything_else() {
        for cmd in [
            "cargo test 2>&1 | tail -40",
            "cat /etc/hosts",
            "cat ../outside",
            "cat .env",
            "ls ~",
            "echo hi > out.txt",
            "find . -delete",
            "find . -exec rm {} ;",
            "rg --pre=sh x",
            "sort -o out in",
            "sort -ro out in",
            "git -c core.pager=sh log",
            "git commit -m x",
            "git diff --output=x",
            "cd /tmp && ls",
            "cd src; ls",
            "ls | sh",
            "xargs cat",
        ] {
            assert!(!allowed(cmd), "allowed: {cmd}");
        }
    }
}
