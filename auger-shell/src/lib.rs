//! Parses bash commands into plain command lists.
//!
//! Only the simplest shape is understood: commands made of literal words,
//! joined by `&&`, `||`, `;` or `|`, with file redirects. Anything else
//! (expansions, substitutions, loops, subshells, assignments, heredocs,
//! background jobs) makes [`plain_commands`] return `None`. This crate does not
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
    pub argv: Vec<Arg>,
    pub redirects: Vec<Redirect>,
}

/// Plain commands joined by `&& || ; |`, in source order, or `None` if
/// anything else appears.
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
}

impl Walker<'_> {
    /// Walk a node whose children are statements and operators.
    fn statements(&mut self, node: Node) -> Option<()> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "comment" => {}
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
            "command" => self.command(node, Vec::new()),
            "redirected_statement" => {
                let body = node.child_by_field_name("body")?;
                if body.kind() != "command" {
                    return None;
                }
                let mut redirects = Vec::new();
                let mut cursor = node.walk();
                for child in node.children_by_field_name("redirect", &mut cursor) {
                    redirects.push(self.redirect(child)?);
                }
                self.command(body, redirects)
            }
            _ => None,
        }
    }

    fn command(&mut self, node: Node, mut redirects: Vec<Redirect>) -> Option<()> {
        let mut argv = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
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
                    expands: raw.contains(['*', '?', '[', '{', '~']),
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
                let mut text = String::new();
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    match child.kind() {
                        "\"" => {}
                        "string_content" => text.push_str(&unescape_dquoted(self.text(child))),
                        _ => return None,
                    }
                }
                Some(Arg {
                    text,
                    expands: false,
                })
            }
            "concatenation" => {
                let mut text = String::new();
                let mut expands = false;
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
    fn marks_expanding_words() {
        let cmds = plain_commands("grep --include=*.rs x ~/y '*'").unwrap();
        let expands: Vec<_> = cmds[0].argv.iter().map(|a| a.expands).collect();
        assert_eq!(expands, [false, true, false, true, false]);
    }

    #[test]
    fn rejects_everything_else() {
        for src in [
            "echo $HOME",
            "echo \"$HOME\"",
            "cat $(ls)",
            "cat `ls`",
            "for f in a; do cat $f; done",
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
