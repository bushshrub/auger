//! Display formatting for shell commands: one statement per line, `&&`/`||`
//! chains broken before the operator, and loop and `if` bodies indented. A
//! `;` that ends a line is implied by the line break and hidden; the words
//! are kept verbatim.

const INDENT: &str = "  ";

/// Format `src` into display lines.
pub fn format(src: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut depth = 0usize;
    for (statement, terminator) in statements(src) {
        let mut rest = statement.as_str();
        loop {
            let (word, tail) = first_word(rest);
            match word {
                // `for ...; do` and `if ...; then` read best on one line.
                "do" | "then" if !out.is_empty() => {
                    if let Some(last) = out.last_mut() {
                        last.push_str("; ");
                        last.push_str(word);
                    }
                    depth += 1;
                }
                "else" => {
                    out.push(format!("{}else", INDENT.repeat(depth.saturating_sub(1))));
                }
                "elif" | "done" | "fi" | "esac" => {
                    depth = depth.saturating_sub(1);
                    emit(&mut out, depth, rest, terminator);
                    break;
                }
                _ => {
                    emit(&mut out, depth, rest, terminator);
                    break;
                }
            }
            rest = tail.trim_start();
            if rest.is_empty() {
                break;
            }
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn emit(out: &mut Vec<String>, depth: usize, statement: &str, terminator: &str) {
    let indent = INDENT.repeat(depth);
    for line in split_ops(statement).iter().flat_map(|step| step.lines()) {
        out.push(format!("{indent}{}", line.trim_end()));
    }
    // `;;` ends a case arm and carries meaning; a plain `;` does not.
    if terminator == ";;" {
        if let Some(last) = out.last_mut() {
            last.push_str(terminator);
        }
    }
}

fn first_word(text: &str) -> (&str, &str) {
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    text.split_at(end)
}

/// Tracks quoting while scanning, so operators inside strings and `$(...)`
/// are left alone.
#[derive(Default)]
struct Scan {
    single: bool,
    double: bool,
    depth: usize,
}

impl Scan {
    /// Feed byte `c`; returns how many bytes to skip after it (escapes).
    fn feed(&mut self, c: u8) -> usize {
        match c {
            b'\\' if !self.single => return 1,
            b'\'' if !self.double => self.single = !self.single,
            b'"' if !self.single => self.double = !self.double,
            b'(' if !self.single && !self.double => self.depth += 1,
            b')' if !self.single && !self.double => self.depth = self.depth.saturating_sub(1),
            _ => {}
        }
        0
    }

    fn top(&self) -> bool {
        !self.single && !self.double && self.depth == 0
    }
}

/// Split at top-level `;`, `;;` and newlines into trimmed statements with
/// their terminator (`""` for a newline or the end). A newline after
/// `&&`, `||` or `|` continues the statement.
fn statements(src: &str) -> Vec<(String, &'static str)> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut scan = Scan::default();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if scan.top() && (c == b';' || c == b'\n') {
            let text = src[start..i].trim();
            let continues = c == b'\n' && (text.ends_with("&&") || text.ends_with('|'));
            if !continues {
                let (terminator, len) = match (c, bytes.get(i + 1)) {
                    (b';', Some(b';')) => (";;", 2),
                    (b';', _) => (";", 1),
                    _ => ("", 1),
                };
                if !text.is_empty() {
                    out.push((text.to_string(), terminator));
                }
                i += len;
                start = i;
                continue;
            }
        }
        i += 1 + scan.feed(c);
    }
    let text = src[start.min(src.len())..].trim();
    if !text.is_empty() {
        out.push((text.to_string(), ""));
    }
    out
}

/// Split a statement before each top-level `&&` and `||`.
fn split_ops(statement: &str) -> Vec<&str> {
    let bytes = statement.as_bytes();
    let mut out = Vec::new();
    let mut scan = Scan::default();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if scan.top() && (c == b'&' || c == b'|') && bytes.get(i + 1) == Some(&c) {
            out.push(statement[start..i].trim());
            start = i;
            i += 2;
            continue;
        }
        i += 1 + scan.feed(c);
    }
    out.push(statement[start..].trim());
    out.retain(|step| !step.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indents_loops_and_ifs() {
        assert_eq!(
            format("for d in a b; do if test -d $d; then echo \"$d; ok\"; else echo no; fi; done"),
            [
                "for d in a b; do",
                "  if test -d $d; then",
                "    echo \"$d; ok\"",
                "  else",
                "    echo no",
                "  fi",
                "done",
            ]
        );
    }

    #[test]
    fn breaks_chains_before_operators() {
        assert_eq!(
            format("cd /a && echo 'x && y' || (b && c) | head; ls"),
            ["cd /a", "&& echo 'x && y'", "|| (b && c) | head", "ls"]
        );
    }
}
