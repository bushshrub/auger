//! Display formatting for shell commands: one statement per line, `&&`/`||`
//! steps on their own lines indented under the first, and loop and `if`
//! bodies indented under their keyword. A `;` that ends a line is implied by
//! the line break and hidden; the words are kept verbatim.

/// Indent of a loop or `if` body past its keyword.
const BODY: usize = 2;
/// Indent of the `&&`/`||` steps after the first.
const STEP_INDENT: &str = "    ";

/// Format `src` into display lines.
pub fn format(src: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Each open `for`/`while`/`if`: its keyword's column and the column of
    // the statement it belongs to.
    let mut open: Vec<(usize, usize)> = Vec::new();
    for (statement, terminator) in statements(src) {
        let mut rest = statement.as_str();
        loop {
            let body = open.last().map_or(0, |(col, _)| col + BODY);
            let (word, tail) = first_word(rest);
            match word {
                // `for ...; do` and `if ...; then` read best on one line.
                "do" | "then" if !out.is_empty() => {
                    if let Some(last) = out.last_mut() {
                        last.push_str("; ");
                        last.push_str(word);
                        open.push(columns(last));
                    }
                }
                "else" => {
                    let col = open.last().map_or(0, |(col, _)| *col);
                    out.push(format!("{}else", " ".repeat(col)));
                }
                "elif" | "done" | "fi" | "esac" => {
                    let (col, base) = open.pop().unwrap_or((0, 0));
                    emit(&mut out, col, base, rest, terminator);
                    break;
                }
                _ => {
                    emit(&mut out, body, body, rest, terminator);
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

/// For an opener line: where its keyword starts (past any leading `&&` or
/// `||`), and the column of the statement the line belongs to.
fn columns(line: &str) -> (usize, usize) {
    let indent = line.len() - line.trim_start().len();
    let trimmed = line.trim_start();
    match trimmed.strip_prefix("&& ").or_else(|| trimmed.strip_prefix("|| ")) {
        Some(_) => (indent + 3, indent.saturating_sub(STEP_INDENT.len())),
        None => (indent, indent),
    }
}

/// Push `statement`: its first step at `column`, later `&&`/`||` steps
/// indented from `base`.
fn emit(out: &mut Vec<String>, column: usize, base: usize, statement: &str, terminator: &str) {
    for (index, step) in split_ops(statement).into_iter().enumerate() {
        let indent = match index {
            0 => " ".repeat(column),
            _ => " ".repeat(base) + STEP_INDENT,
        };
        for line in step.lines() {
            out.push(format!("{indent}{}", line.trim_end()));
        }
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
    fn loops_inside_a_step_indent_under_the_keyword() {
        assert_eq!(
            format("cd /a && for d in x; do echo $d; done && ls"),
            ["cd /a", "    && for d in x; do", "         echo $d", "       done", "    && ls"]
        );
    }

    #[test]
    fn indents_chain_steps() {
        assert_eq!(
            format("cd /a && echo 'x && y' || (b && c) | head; ls"),
            ["cd /a", "    && echo 'x && y'", "    || (b && c) | head", "ls"]
        );
    }
}
