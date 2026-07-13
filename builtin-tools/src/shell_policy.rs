use agent_core::AutoApprovalPolicy;
use provider::ToolCallRequest;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use yash_syntax::syntax::{
    Command, List, SimpleCommand, Text, TextUnit, Word, WordUnit,
};

/// Conservative auto-approval policy for the `/bin/sh -c` shell tool.
pub struct BashAutoApprovalPolicy {
    cwd: PathBuf,
}

impl BashAutoApprovalPolicy {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: normalize_path(&cwd.into()),
        }
    }
}

impl AutoApprovalPolicy for BashAutoApprovalPolicy {
    fn is_approved(&self, tool_call: &ToolCallRequest) -> bool {
        if tool_call.name != "shell" {
            return false;
        }

        let Ok(arguments) = serde_json::from_str::<Value>(&tool_call.arguments) else {
            return false;
        };
        let Some(command) = arguments.get("command").and_then(Value::as_str) else {
            return false;
        };
        let Ok(list) = command.parse::<List>() else {
            return false;
        };

        validate_list(&list, &self.cwd)
    }
}

fn validate_list(list: &List, cwd: &Path) -> bool {
    if list.0.len() != 1 {
        return false;
    }

    let item = &list.0[0];
    if item.async_flag.is_some() {
        return false;
    }

    let and_or = item.and_or.as_ref();
    let standalone = and_or.rest.is_empty() && and_or.first.commands.len() == 1;

    validate_pipeline(&and_or.first, cwd, standalone)
        && and_or
            .rest
            .iter()
            .all(|(_, pipeline)| validate_pipeline(pipeline, cwd, false))
}

fn validate_pipeline(
    pipeline: &yash_syntax::syntax::Pipeline,
    cwd: &Path,
    standalone: bool,
) -> bool {
    if pipeline.negation || pipeline.commands.is_empty() {
        return false;
    }

    pipeline
        .commands
        .iter()
        .all(|command| validate_command(command, cwd, standalone))
}

fn validate_command(command: &Command, cwd: &Path, standalone: bool) -> bool {
    let Command::Simple(simple) = command else {
        return false;
    };

    let Some(words) = simple_words(simple, true) else {
        return false;
    };
    let Some(name) = words.first() else {
        return false;
    };
    let args = &words[1..];

    match name.as_str() {
        "pwd" => args.is_empty(),
        "ls" => validate_ls(args, cwd),
        "grep" => validate_grep(args, cwd, false),
        "rg" => validate_grep(args, cwd, true),
        "find" => standalone && validate_find(args, cwd),
        _ => false,
    }
}

fn simple_words(simple: &SimpleCommand, reject_glob: bool) -> Option<Vec<String>> {
    if !simple.assigns.is_empty() || !simple.redirs.is_empty() {
        return None;
    }

    simple
        .words
        .iter()
        .map(|(word, _)| word_to_string(word, reject_glob))
        .collect()
}

fn word_to_string(word: &Word, reject_glob: bool) -> Option<String> {
    let mut result = String::new();

    for unit in &word.units {
        match unit {
            WordUnit::Unquoted(text) => append_text_unit(text, &mut result, reject_glob)?,
            WordUnit::SingleQuote(value) => result.push_str(value),
            WordUnit::DoubleQuote(text) => append_text(text, &mut result)?,
            WordUnit::DollarSingleQuote(_) | WordUnit::Tilde { .. } => return None,
        }
    }

    Some(result)
}

fn append_text(text: &Text, result: &mut String) -> Option<()> {
    for unit in &text.0 {
        append_text_unit(unit, result, false)?;
    }
    Some(())
}

fn append_text_unit(unit: &TextUnit, result: &mut String, reject_glob: bool) -> Option<()> {
    match unit {
        TextUnit::Literal(c) => {
            if reject_glob && matches!(c, '*' | '?' | '[') {
                return None;
            }
            result.push(*c);
        }
        TextUnit::Backslashed(c) => result.push(*c),
        TextUnit::RawParam { .. }
        | TextUnit::BracedParam(_)
        | TextUnit::CommandSubst { .. }
        | TextUnit::Backquote { .. }
        | TextUnit::Arith { .. } => return None,
    }
    Some(())
}

fn validate_ls(args: &[String], cwd: &Path) -> bool {
    const LONG_OPTIONS: &[&str] = &[
        "--all",
        "--almost-all",
        "--classify",
        "--directory",
        "--human-readable",
        "--long",
        "--one-file-system",
        "--recursive",
        "--reverse",
    ];
    const SHORT_OPTIONS: &[char] = &['1', 'A', 'a', 'd', 'F', 'h', 'l', 'R', 'r', 'S', 't', 'U'];

    let mut paths_started = false;
    for arg in args {
        if arg == "--" {
            paths_started = true;
            continue;
        }
        if !paths_started && arg.starts_with("--") {
            if !LONG_OPTIONS.contains(&arg.as_str()) {
                return false;
            }
            continue;
        }
        if !paths_started && arg.starts_with('-') && arg != "-" {
            if !short_option_bundle(arg, SHORT_OPTIONS) {
                return false;
            }
            continue;
        }
        paths_started = true;
        if !path_is_within(cwd, arg) {
            return false;
        }
    }
    true
}

fn validate_grep(args: &[String], cwd: &Path, ripgrep: bool) -> bool {
    const GREP_OPTIONS: &[char] = &['E', 'F', 'G', 'H', 'L', 'R', 'c', 'h', 'i', 'l', 'n', 'o', 'q', 'r', 's', 'v', 'w', 'x'];
    const RG_OPTIONS: &[char] = &['F', 'H', 'L', 'c', 'h', 'i', 'l', 'n', 'o', 'q', 's', 'v', 'w', 'x'];
    let allowed_options = if ripgrep { RG_OPTIONS } else { GREP_OPTIONS };

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if arg.starts_with('-') {
            if arg == "-" || !short_option_bundle(arg, allowed_options) {
                return false;
            }
            index += 1;
            continue;
        }
        break;
    }

    if index >= args.len() {
        return false;
    }
    index += 1;

    args[index..].iter().all(|path| {
        path != "-" && !path.starts_with('-') && path_is_within(cwd, path)
    })
}

fn validate_find(args: &[String], cwd: &Path) -> bool {
    let mut index = 0;
    while index < args.len() && !find_expression_start(&args[index]) {
        if !path_is_within(cwd, &args[index]) {
            return false;
        }
        index += 1;
    }

    while index < args.len() {
        let predicate = &args[index];
        index += 1;
        match predicate.as_str() {
            "-name" | "-iname" | "-path" | "-ipath" | "-regex" | "-iregex" | "-printf" => {
                if index >= args.len() || args[index].is_empty() {
                    return false;
                }
                index += 1;
            }
            "-type" => {
                if index >= args.len()
                    || !matches!(args[index].as_str(), "b" | "c" | "d" | "f" | "l" | "p" | "s")
                {
                    return false;
                }
                index += 1;
            }
            "-maxdepth" | "-mindepth" => {
                if index >= args.len() || args[index].parse::<usize>().is_err() {
                    return false;
                }
                index += 1;
            }
            "-a" | "-and" | "-o" | "-or" | "!" | "-not" | "(" | ")" | "-xdev"
            | "-mount" | "-depth" | "-d" | "-prune" | "-print" | "-print0" | "-ls"
            | "-empty" | "-readable" | "-writable" | "-executable" => {}
            _ => return false,
        }
    }

    true
}

fn find_expression_start(arg: &str) -> bool {
    arg.starts_with('-') || matches!(arg, "!" | "(" | ")")
}

fn short_option_bundle(arg: &str, allowed: &[char]) -> bool {
    let mut chars = arg.chars();
    if chars.next() != Some('-') || chars.next() == Some('-') {
        return false;
    }
    chars.all(|c| allowed.contains(&c))
}

fn path_is_within(cwd: &Path, value: &str) -> bool {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return false;
    }

    normalize_path(&cwd.join(path)).starts_with(cwd)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy() -> BashAutoApprovalPolicy {
        BashAutoApprovalPolicy::new("/workspace/project")
    }

    fn approved(command: &str) -> bool {
        let call = ToolCallRequest {
            id: "test".into(),
            name: "shell".into(),
            arguments: json!({ "command": command }).to_string(),
        };
        policy().is_approved(&call)
    }

    fn approved_with_args(arguments: &str) -> bool {
        let call = ToolCallRequest {
            id: "test".into(),
            name: "shell".into(),
            arguments: arguments.into(),
        };
        policy().is_approved(&call)
    }

    #[test]
    fn approves_safe_commands_and_composition() {
        assert!(approved("pwd"));
        assert!(approved("ls -la src"));
        assert!(approved("grep -R 'needle' src"));
        assert!(approved("rg 'needle' src"));
        assert!(approved("find . -name '*.rs'"));
        assert!(approved("pwd && ls . | rg 'src'"));
        assert!(approved("grep 'a&&b' './file'"));
    }

    #[test]
    fn rejects_shell_control_and_expansion_syntax() {
        for command in [
            "pwd; ls",
            "pwd && find .",
            "find . | rg rs",
            "x=$(pwd)",
            "echo `pwd`",
            "(pwd)",
            "A=1 pwd",
            "pwd > output",
            "pwd &",
            "rg $PATTERN src",
            "rg *.rs src",
            "xargs rg rs",
            "find . -exec cat {} \\;",
        ] {
            assert!(!approved(command), "unexpected approval: {command}");
        }
    }

    #[test]
    fn rejects_unsafe_commands_options_and_paths() {
        for command in [
            "/bin/pwd",
            "ls /workspace",
            "ls ../other",
            "grep --include '*.rs' rs .",
            "rg --pre cat rs .",
            "find . -delete",
            "find . -fprint output",
        ] {
            assert!(!approved(command), "unexpected approval: {command}");
        }
    }

    #[test]
    fn rejects_malformed_arguments() {
        assert!(!approved_with_args("{"));
        assert!(!approved_with_args("{}"));
        assert!(!approved_with_args(r#"{"command": 42}"#));
    }
}
