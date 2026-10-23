//! Allows bash commands that only read inside the workspace.

use crate::paths::inside_workspace;
use crate::paths::is_secret;
use crate::paths::resolve;
use auger_toolengine::PolicyCall;
use auger_toolengine::PolicyNode;
use auger_toolengine::Verdict;
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
    /// Every non-flag argument must be a readable path, and no argument may
    /// be one of the `long` flags (or an abbreviation of one, which GNU
    /// tools accept) or a short-flag cluster holding one of the `short`
    /// letters.
    Paths {
        long: &'static [&'static str],
        short: &'static str,
    },
}

const NONE: Rule = Rule::Paths { long: &[], short: "" };

/// Single-dash `find` primaries, matched whole.
const FIND_BANNED: &[&str] = &[
    "-delete", "-ok", "-okdir", "-fls", "-fprint", "-fprint0", "-fprintf",
    // Following symlinks can lead out of the workspace, and xargs trusts
    // find's output to stay inside it.
    "-L", "-follow",
];
const GIT_BANNED: &[&str] = &["--output", "--ext-diff", "--textconv"];
const GIT_SUBCOMMANDS: &[&str] = &[
    "status", "diff", "log", "show", "rev-parse", "ls-files", "blame", "shortlog",
];

fn rule(program: &str) -> Option<Rule> {
    Some(match program {
        "echo" | "printf" | "true" | "pwd" => Rule::Free,
        "cat" | "head" | "tail" | "ls" | "stat" | "which" | "basename" | "dirname"
        | "realpath" | "diff" | "cmp" | "jq" | "nl" | "cut" | "tr" => NONE,
        // Reading the list of names from a file can name anything.
        "wc" => Rule::Paths { long: &["--files0-from"], short: "" },
        // -R follows every symlink, out of the workspace too.
        "grep" | "egrep" | "fgrep" => Rule::Paths { long: &["--dereference-recursive"], short: "R" },
        "du" => Rule::Paths { long: &["--dereference", "--files0-from"], short: "L" },
        // -l follows symlinks, -o writes the listing to a file.
        "tree" => Rule::Paths { long: &[], short: "lo" },
        // -C compiles a magic file, which writes one.
        "file" => Rule::Paths { long: &["--compile"], short: "C" },
        "rg" => Rule::Paths {
            long: &["--pre", "--pre-glob", "--hostname-bin", "--follow"],
            short: "L",
        },
        "sort" => Rule::Paths {
            long: &["--output", "--compress-program", "--files0-from"],
            short: "o",
        },
        _ => return None,
    })
}

fn read_only(commands: &[Command], cwd: &Path, workspace: &Path) -> bool {
    if !inside_workspace(workspace, cwd, Path::new("")) {
        return false;
    }
    // Every directory a command may run in: a `cd` that can fail leaves the
    // old directory possible too, so commands are checked against all.
    let mut cwds = vec![cwd.to_path_buf()];
    for (i, command) in commands.iter().enumerate() {
        if command.argv[0].text == "cd" {
            match cd_targets(commands, i, &cwds, workspace) {
                Some(next) => cwds = next,
                None => return false,
            }
            continue;
        }
        let ok = cwds.iter().all(|cwd| {
            command.redirects.iter().all(|r| redirect_ok(r, cwd, workspace))
                && match command.argv[0].text.as_str() {
                    "xargs" => xargs_ok(commands, i, cwd, workspace),
                    _ => command_ok(&command.argv, cwd, workspace),
                }
        });
        if !ok {
            return false;
        }
    }
    true
}

/// The directories possible after the `cd <dir>` at `i`; each must be inside
/// the workspace. Only the new ones remain when nothing can run after a failed
/// `cd`: it is the first command and every later top-level step is joined by
/// `&&` or `|`. Otherwise the old ones stay possible as well.
fn cd_targets(commands: &[Command], i: usize, cwds: &[PathBuf], workspace: &Path) -> Option<Vec<PathBuf>> {
    let command = &commands[i];
    let [_, dir] = command.argv.as_slice() else {
        return None;
    };
    if dir.expands || command.depth > 0 || !command.redirects.is_empty() {
        return None;
    }
    let mut targets = Vec::new();
    for cwd in cwds {
        let target = resolve(cwd, Path::new(&dir.text))?;
        if !inside_workspace(workspace, cwd, &target) {
            return None;
        }
        targets.push(target);
    }
    let guarded = i == 0
        && commands.get(1).is_none_or(|c| c.joint == Some(Joint::And))
        && commands
            .iter()
            .skip(1)
            .filter(|c| c.depth == 0 || c.joint == Some(Joint::Or))
            .all(|c| matches!(c.joint, Some(Joint::And | Joint::Pipe)));
    if !guarded {
        targets.extend(cwds.iter().cloned());
    }
    Some(targets)
}


/// `find ... | xargs <read-only program>`: the names xargs appends come from
/// `find`, whose roots were checked, so they stay inside the workspace. Only
/// programs that just read their path arguments may run this way.
fn xargs_ok(commands: &[Command], i: usize, cwd: &Path, workspace: &Path) -> bool {
    let command = &commands[i];
    let fed_by_find = i > 0
        && command.joint == Some(Joint::Pipe)
        && commands[i - 1].argv[0].text == "find";
    let inner = command.argv[1..]
        .iter()
        .position(|a| !matches!(a.text.as_str(), "-0" | "-r" | "--null" | "--no-run-if-empty"))
        .map(|start| &command.argv[1 + start..]);
    match inner {
        Some(inner) if fed_by_find => {
            matches!(rule(&inner[0].text), Some(Rule::Paths { .. }))
                && command_ok(inner, cwd, workspace)
        }
        _ => false,
    }
}

/// `find` with `-exec`/`-execdir <read-only program> ... {} ;` or `+`. The
/// names `{}` stands for come from find's roots, which are checked like any
/// path, so the inner program only needs to be one that just reads its path
/// arguments. Everything outside the `-exec` clauses follows the find rule.
fn find_ok(args: &[Arg], cwd: &Path, workspace: &Path) -> bool {
    let mut outer = Vec::new();
    let mut rest = args;
    while let Some(start) = rest.iter().position(|a| matches!(a.text.as_str(), "-exec" | "-execdir")) {
        outer.extend_from_slice(&rest[..start]);
        let clause = &rest[start + 1..];
        let Some(end) = clause.iter().position(|a| a.text == ";" || a.text == "+") else {
            return false;
        };
        let inner: Vec<Arg> = clause[..end].iter().filter(|a| a.text != "{}").cloned().collect();
        let read_only = inner
            .first()
            .is_some_and(|program| matches!(rule(&program.text), Some(Rule::Paths { .. })));
        if !read_only || !command_ok(&inner, cwd, workspace) {
            return false;
        }
        rest = &clause[end + 1..];
    }
    outer.extend_from_slice(rest);
    outer.iter().all(|a| !FIND_BANNED.contains(&a.text.as_str()) && path_ok(a, cwd, workspace))
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
    if program.text == "sed" {
        return sed_ok(args, cwd, workspace);
    }
    if program.text == "find" {
        return find_ok(args, cwd, workspace);
    }
    match rule(&program.text) {
        Some(Rule::Free) => true,
        Some(Rule::Paths { long, short }) => args.iter().all(|a| {
            !is_banned(&a.text, long) && !in_cluster(&a.text, short) && path_ok(a, cwd, workspace)
        }),
        None => false,
    }
}

/// `git <read-only subcommand> ...`. Global options before the subcommand
/// (`-c`, `-C`, `--git-dir`) can run arbitrary programs, so none are allowed.
fn git_ok(args: &[Arg], cwd: &Path, workspace: &Path) -> bool {
    let Some((subcommand, rest)) = args.split_first() else {
        return false;
    };
    // `git branch` with only listing flags; a name would create a branch.
    if subcommand.text == "branch" {
        return rest
            .iter()
            .all(|a| matches!(a.text.as_str(), "-a" | "-r" | "-v" | "-vv" | "--all" | "--list"));
    }
    GIT_SUBCOMMANDS.contains(&subcommand.text.as_str())
        && rest.iter().all(|a| !is_banned(&a.text, GIT_BANNED) && path_ok(a, cwd, workspace))
}

/// `sed -n '<lines>p' files...`: printing a line range only. Any other sed
/// script can write files (`w`) or run commands (`e`).
fn sed_ok(args: &[Arg], cwd: &Path, workspace: &Path) -> bool {
    let [quiet, script, files @ ..] = args else {
        return false;
    };
    let range = script.text.strip_suffix('p').unwrap_or("");
    quiet.text == "-n"
        && !range.is_empty()
        && range.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '$')
        && files.iter().all(|a| !a.expands && !a.text.starts_with('-') && path_ok(a, cwd, workspace))
}

/// Whether `arg` is one of the long flags in `banned`, with or without
/// `=value`, or an abbreviation of one: GNU tools and git accept any unique
/// prefix, so `--outp=x` means `--output=x`.
fn is_banned(arg: &str, banned: &[&str]) -> bool {
    let Some(name) = arg.split('=').next().filter(|n| n.starts_with("--") && n.len() > 2) else {
        return false;
    };
    banned.iter().any(|flag| flag.starts_with(name))
}

/// Whether `arg` is a cluster of short flags (`-rn`) holding any of `letters`.
fn in_cluster(arg: &str, letters: &str) -> bool {
    arg.len() > 1
        && arg.starts_with('-')
        && !arg.starts_with("--")
        && arg[1..].contains(|c| letters.contains(c))
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
    if arg.expands && !globs_ok(text, cwd, workspace) {
        return false;
    }
    let path = Path::new(text);
    inside_workspace(workspace, cwd, path)
        && !is_secret(path)
        && resolve(cwd, path).is_some_and(|p| !is_secret(&p))
}

/// An unquoted pattern is checked by what bash would expand it to: every
/// match must be inside the workspace, not a secret, and not look like a flag.
/// `~` and brace expansion are not worked out and always fail.
fn globs_ok(pattern: &str, cwd: &Path, workspace: &Path) -> bool {
    if pattern.starts_with('~') || (pattern.contains('{') && (pattern.contains(',') || pattern.contains(".."))) {
        return false;
    }
    let Some(full) = cwd.join(pattern).to_str().map(str::to_owned) else {
        return false;
    };
    let Ok(matches) = glob::glob(&full) else {
        return false;
    };
    matches.into_iter().all(|m| {
        m.is_ok_and(|path| {
            let flag_like = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('-'));
            !flag_like
                && inside_workspace(workspace, cwd, &path)
                && resolve(cwd, &path).is_some_and(|p| !is_secret(&p))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(cmd: &str) -> bool {
        let ws = std::env::temp_dir().join("auger-bash-ro-test");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        // A file whose name is a flag, and a symlink leading out.
        std::fs::write(ws.join("src/a.rs"), "").unwrap();
        std::fs::write(ws.join("src/--pre=sh"), "").unwrap();
        let _ = std::os::unix::fs::symlink("/etc", ws.join("link"));
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
            "cd $WS && git log --oneline -15 2>/dev/null; echo ---; git branch -a 2>/dev/null; git status --short | head -30",
            "cd src; cat a.rs",
            r"find src -name '*.rs' -exec wc -l {} \; | sort -n",
            "find . -type f -exec grep -n todo {} +",
            "cat src/*.rs",
            "grep -rn x --include=*.rs src",
            "git branch -a && git shortlog -sn | head",
            r#"cd $WS && for f in src/a.rs src/b.rs; do echo "=== $f ==="; cat "$f"; done"#,
            r#"cd $WS && for d in src; do echo "=== $d ==="; find $d -name '*.rs' 2>/dev/null | xargs wc -l | sort -n | tail -30; done"#,
            "cd $WS && sed -n 110,180p src/a.rs && echo '=== x ===' && head -70 src/b.rs",
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
            "find . -exec sh -c id {} +",
            "find . -exec cat {} ; -delete",
            "find . -exec cat /etc/hosts {} +",
            "find . -exec cat {}",
            "rg --pre=sh x",
            "sort -o out in",
            "sort -ro out in",
            "sort --outp=x in",
            "sort --compress-prog=sh in",
            "git diff --out=x",
            "grep -rR x .",
            "grep --dereference-rec x .",
            "rg -L x",
            "rg --follow x",
            "tree -o out",
            "file -C -m magic",
            "wc --files0-from=list",
            "cat {/etc/hosts,x}",
            "cat link*",
            "cat lin?/hosts",
            "rg x src/*",
            "cat link/hosts",
            "git -c core.pager=sh log",
            "git commit -m x",
            "git branch new-feature",
            "for f in src/a.rs /etc/hosts; do cat $f; done",
            "for f in src/a.rs; do rm $f; done",
            "git branch -D main",
            "git diff --output=x",
            "cd /tmp && ls",
            "cd src; cat ../../outside",
            "ls | sh",
            "xargs cat",
            "echo /etc/hosts | xargs cat",
            "find . -name x | xargs rm",
            "find -L . -name x | xargs cat",
            "find . -name x | xargs sh -c id",
            "sed -i s/a/b/ src/a.rs",
            "sed -n 1w/tmp/x src/a.rs",
            "sed -n 1p /etc/hosts",
        ] {
            assert!(!allowed(cmd), "allowed: {cmd}");
        }
    }
}
