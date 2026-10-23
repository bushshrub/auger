//! Parses bash commands into plain command lists.
//!
//! Only the simplest shape is understood: commands made of literal words,
//! joined by `&&`, `||`, `;` or `|`, with file redirects, plus `for` loops
//! over literal words, which are unrolled. Anything else (other expansions,
//! substitutions, subshells, assignments, heredocs, background jobs) makes
//! [`plain_commands`] return `None`. This crate does not
//! judge whether a command is safe; it only tells callers what will run.

use tree_sitter::Node;
use tree_sitter::Parser;

/// How a command is joined to the one before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Joint {
    And,
    Or,
    Seq,
    Pipe,
}

/// One word of a command after quote removal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arg {
    pub text: String,
    /// The word holds unquoted `* ? [ { ~`, so bash may expand it into
    /// something else (file names, the home directory).
    pub expands: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    pub fd: Option<u32>,
    /// The operator as written, e.g. `>`, `>>`, `<`, `>&`, `&>`.
    pub op: String,
    pub target: Arg,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// `None` for the first command.
    pub joint: Option<Joint>,
    /// How many `for` loops this command sits in. Joints inside a loop only
    /// order the loop's own commands.
    pub depth: usize,
    pub argv: Vec<Arg>,
    pub redirects: Vec<Redirect>,
}

/// Most items a `for` loop may unroll to.
const MAX_UNROLL: usize = 64;

/// Plain commands joined by `&& || ; |`, in source order, or `None` if
/// anything else appears. A `for` loop over literal words is unrolled, with
/// the loop variable substituted in its body.
pub fn plain_commands(src: &str) -> Option<Vec<Command>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(src, None)?;
    let root = tree.root_node();
    if root.has_error() || root.kind() != "program" {
        return None;
    }
    let mut walker = Walker {
        src,
        joint: None,
        out: Vec::new(),
        vars: Vec::new(),
    };
    walker.statements(root)?;
    if walker.out.is_empty() {
        return None;
    }
    Some(walker.out)
}

struct Walker<'s> {
    src: &'s str,
    /// The operator seen since the last command.
    joint: Option<Joint>,
    out: Vec<Command>,
    /// Bound loop variables, innermost last.
    vars: Vec<(String, String)>,
}

impl Walker<'_> {
    /// Walk a node whose children are statements and operators.
    fn statements(&mut self, node: Node) -> Option<()> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "comment" | "do" | "done" => {}
                "&&" => self.joint = Some(Joint::And),
                "||" => self.joint = Some(Joint::Or),
                "|" => self.joint = Some(Joint::Pipe),
                ";" | "\n" => self.joint = Some(Joint::Seq),
                _ if child.is_named() => self.statement(child)?,
                _ => return None,
            }
        }
        Some(())
    }

    fn statement(&mut self, node: Node) -> Option<()> {
        match node.kind() {
            "list" | "pipeline" => self.statements(node),
            "for_statement" => self.for_loop(node),
            "command" => self.command(node, Vec::new()),
            "redirected_statement" => {
                let body = node.child_by_field_name("body")?;
                let mut redirects = Vec::new();
                let mut cursor = node.walk();
                for child in node.children_by_field_name("redirect", &mut cursor) {
                    redirects.push(self.redirect(child)?);
                }
                if body.kind() == "command" {
                    return self.command(body, redirects);
                }
                // tree-sitter wraps a whole `a && b 2>x` list or pipeline in
                // the redirect, but bash binds it to the last command only.
                if !matches!(body.kind(), "list" | "pipeline") {
                    return None;
                }
                let before = self.out.len();
                self.statement(body)?;
                if self.out.len() == before {
                    return None;
                }
                self.out.last_mut()?.redirects.extend(redirects);
                Some(())
            }
            _ => None,
        }
    }

    /// `for NAME in WORDS; do BODY; done`, unrolled: BODY once per word with
    /// NAME bound to it.
    fn for_loop(&mut self, node: Node) -> Option<()> {
        let mut name = None;
        let mut values = Vec::new();
        let mut body = None;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "for" | "in" | ";" | "\n" => {}
                "variable_name" => name = Some(self.text(child).to_string()),
                "do_group" => body = Some(child),
                _ => {
                    let value = self.word(child)?;
                    if value.expands {
                        return None;
                    }
                    values.push(value.text);
                }
            }
        }
        // `for f; do` loops over the arguments, which we can't see.
        let (name, body) = (name?, body?);
        if values.is_empty() || values.len() > MAX_UNROLL {
            return None;
        }
        for value in values {
            self.vars.push((name.clone(), value));
            self.statements(body)?;
            self.vars.pop();
        }
        Some(())
    }

    fn lookup(&self, node: Node) -> Option<&str> {
        let name = match node.kind() {
            "simple_expansion" => node.named_child(0)?,
            // Only plain `${name}`, no operators.
            "expansion" if node.named_child_count() == 1 => node.named_child(0)?,
            _ => return None,
        };
        if name.kind() != "variable_name" {
            return None;
        }
        let name = self.text(name);
        self.vars.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn command(&mut self, node: Node, mut redirects: Vec<Redirect>) -> Option<()> {
        let mut argv = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            // An unnamed token here (a lone `$`) is something we don't
            // model; skipping it would drop an argument bash passes.
            if !child.is_named() {
                return None;
            }
            match child.kind() {
                "command_name" => argv.push(self.word(child.named_child(0)?)?),
                "file_redirect" => redirects.push(self.redirect(child)?),
                _ => argv.push(self.word(child)?),
            }
        }
        if argv.is_empty() {
            return None;
        }
        // `prev ; cmd` and a first command at the top level look the same to
        // the walker, so only the very first command has no joint.
        let joint = if self.out.is_empty() {
            None
        } else {
            Some(self.joint.unwrap_or(Joint::Seq))
        };
        self.joint = None;
        self.out.push(Command {
            joint,
            depth: self.vars.len(),
            argv,
            redirects,
        });
        Some(())
    }

    fn redirect(&self, node: Node) -> Option<Redirect> {
        if node.kind() != "file_redirect" {
            return None;
        }
        let mut fd = None;
        let mut op = None;
        let mut target = None;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "file_descriptor" => fd = Some(self.text(child).parse().ok()?),
                _ if !child.is_named() => op = Some(self.text(child).to_string()),
                _ if target.is_none() => target = Some(self.word(child)?),
                _ => return None,
            }
        }
        Some(Redirect {
            fd,
            op: op?,
            target: target?,
        })
    }

    /// Quote removal for a literal word. `None` for anything bash would
    /// substitute.
    fn word(&self, node: Node) -> Option<Arg> {
        match node.kind() {
            "word" | "number" => {
                let raw = self.text(node);
                Some(Arg {
                    text: unescape(raw),
                    expands: expands(raw),
                })
            }
            "raw_string" => {
                let raw = self.text(node);
                Some(Arg {
                    text: raw.get(1..raw.len().checked_sub(1)?)?.to_string(),
                    expands: false,
                })
            }
            "string" => {
                // Walk by byte range: tree-sitter leaves some content (a
                // newline) outside any child, and it must not be lost.
                let (start, end) = (node.start_byte() + 1, node.end_byte().checked_sub(1)?);
                let mut text = String::new();
                let mut pos = start;
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    text.push_str(&unescape_dquoted(self.src.get(pos..child.start_byte())?));
                    match child.kind() {
                        "string_content" => text.push_str(&unescape_dquoted(self.text(child))),
                        _ => text.push_str(self.lookup(child)?),
                    }
                    pos = child.end_byte();
                }
                text.push_str(&unescape_dquoted(self.src.get(pos..end.max(pos))?));
                Some(Arg {
                    text,
                    expands: false,
                })
            }
            // An unquoted loop variable splits on spaces and globs, so its
            // value only counts as one literal word if it has neither.
            "simple_expansion" | "expansion" => {
                let value = self.lookup(node)?;
                Some(Arg {
                    text: value.to_string(),
                    expands: value.contains(|c: char| c.is_whitespace() || "*?[{~".contains(c)),
                })
            }
            "concatenation" => {
                let mut text = String::new();
                // Brace expansion can span the parts: `a{b,c}`.
                let mut expands = expands(self.text(node));
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    let part = self.word(child)?;
                    text.push_str(&part.text);
                    expands |= part.expands;
                }
                Some(Arg { text, expands })
            }
            _ => None,
        }
    }

    fn text(&self, node: Node) -> &str {
        &self.src[node.byte_range()]
    }
}

/// Whether bash may expand an unquoted word: globs, `~`, and brace
/// expansion, which needs a comma or a range (`{}` alone is literal).
fn expands(raw: &str) -> bool {
    raw.contains(['*', '?', '[', '~'])
        || (raw.contains('{') && (raw.contains(',') || raw.contains("..")))
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

/// Inside double quotes a backslash only escapes `$ \` " \` and newline.
fn unescape_dquoted(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some('$' | '`' | '"' | '\\')) => out.extend(chars.next()),
            ('\\', Some('\n')) => {
                chars.next();
            }
            (c, _) => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(src: &str) -> Vec<Vec<String>> {
        plain_commands(src)
            .unwrap_or_else(|| panic!("rejected: {src}"))
            .into_iter()
            .map(|c| c.argv.into_iter().map(|a| a.text).collect())
            .collect()
    }

    #[test]
    fn splits_lists_and_pipelines() {
        let cmds = plain_commands("cd /a && cat x | head -5; ls").unwrap();
        let joints: Vec<_> = cmds.iter().map(|c| c.joint).collect();
        assert_eq!(
            joints,
            [None, Some(Joint::And), Some(Joint::Pipe), Some(Joint::Seq)]
        );
        assert_eq!(argv("cd /a && cat x | head -5; ls")[2], ["head", "-5"]);
    }

    #[test]
    fn removes_quotes() {
        assert_eq!(
            argv(r#"grep -rn "todo!\|x" 'a b' c\ d"#),
            [["grep", "-rn", r"todo!\|x", "a b", "c d"]]
        );
    }

    #[test]
    fn keeps_redirects() {
        let cmds = plain_commands("cargo check 2>&1 | tail -40").unwrap();
        let r = &cmds[0].redirects[0];
        assert_eq!((r.fd, r.op.as_str(), r.target.text.as_str()), (Some(2), ">&", "1"));
    }

    #[test]
    fn redirects_after_a_list_bind_to_its_last_command() {
        let cmds = plain_commands("cd a && git log 2>/dev/null").unwrap();
        assert!(cmds[0].redirects.is_empty());
        assert_eq!(cmds[1].redirects[0].target.text, "/dev/null");
    }

    #[test]
    fn marks_expanding_words() {
        let cmds = plain_commands("grep --include=*.rs x ~/y '*' {} a{b,c}").unwrap();
        let expands: Vec<_> = cmds[0].argv.iter().map(|a| a.expands).collect();
        assert_eq!(expands, [false, true, false, true, false, false, true]);
    }

    #[test]
    fn unrolls_loops_over_literal_words() {
        assert_eq!(
            argv(r#"for f in a.md "b c"; do echo "=== $f ==="; cat "$f" ${f}x; done"#),
            [
                vec!["echo", "=== a.md ==="],
                vec!["cat", "a.md", "a.mdx"],
                vec!["echo", "=== b c ==="],
                vec!["cat", "b c", "b cx"],
            ]
        );
        let cmds = plain_commands("cd /a && for f in x; do cat $f; done").unwrap();
        assert_eq!(cmds.iter().map(|c| c.depth).collect::<Vec<_>>(), [0, 1]);
    }

    /// What we pass on must be exactly what bash passes: a policy that checks
    /// a different word than bash runs can be bypassed.
    #[test]
    fn words_match_bash() {
        let cases = [
            r#"a "b c" 'd e' f\ g"#, r#"a "x\"y" 'it'"'"'s'"#, r#"a "\$HOME" '\n' \\x"#,
            r#"a "a\\b" "a\b" "a\`b""#, r#"a b"c"d 'e'f"#, r#"a --x=*.rs"#, r#"a \*"#, r#"a "*""#,
            r#"a {}"#, r#"a x#y #z"#, "a \"multi\nline\"", "a \\\nb", r#"a 'a\'"#, r#"a "" ''"#,
            r#"a =x x=y !x %s"#, r#"a "é" ü"#, r#"a \"q\""#, r#"a "$""#, r#"a 'x'\''y'"#,
            r#"a -- -x"#, r#"a "\\" '\\'"#, "a \"tab\there\"",
        ];
        for src in cases {
            let Some(cmds) = plain_commands(src) else { continue };
            let ours: Vec<String> = cmds[0].argv[1..].iter().map(|a| a.text.clone()).collect();
            let script = format!("printf '%s\\0' {}", &src[2..]);
            let out = std::process::Command::new("bash").arg("-c").arg(&script).output().unwrap();
            let mut bash: Vec<String> =
                String::from_utf8_lossy(&out.stdout).split('\0').map(String::from).collect();
            bash.pop();
            assert_eq!(ours, bash, "for {src:?}");
        }
    }

    #[test]
    fn rejects_everything_else() {
        for src in [
            "echo $HOME",
            "echo \"$HOME\"",
            "cat $(ls)",
            "cat `ls`",
            "for f in *.md; do cat $f; done",
            "for f in $(ls); do cat $f; done",
            "for f; do cat $f; done",
            "for f in a; do cat $g; done",
            "(cd /a; ls)",
            "{ ls; }",
            "FOO=1 ls",
            "sleep 1 &",
            "cat <<EOF\nx\nEOF",
            "diff <(ls a) <(ls b)",
            "echo 'unclosed",
            "",
        ] {
            assert!(plain_commands(src).is_none(), "accepted: {src}");
        }
    }
}
