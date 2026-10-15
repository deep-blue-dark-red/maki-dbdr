const READ_ONLY_COMMANDS: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "grep",
    "rg",
    "fd",
    "find",
    "wc",
    "cut",
    "sort",
    "uniq",
    "tr",
    "awk",
    "sed",
    "diff",
    "echo",
    "printf",
    "pwd",
    "cd",
    "which",
    "type",
    "file",
    "stat",
    "du",
    "df",
    "ps",
    "pgrep",
    "jq",
    "column",
    "date",
    "env",
    "sleep",
    "true",
    "false",
    "test",
    "[",
    "[[",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "tree",
    "less",
    "more",
    "nl",
    "od",
    "xxd",
    "hexdump",
    "sha256sum",
    "md5sum",
    "shasum",
    "bat",
    "export",
    "set",
    "read",
    "wait",
    "id",
    "whoami",
    "uname",
    "hostname",
    "tput",
    "seq",
    "comm",
    "paste",
    "rev",
    "tac",
    "fold",
    "expand",
    "git status",
    "git diff",
    "git log",
    "git show",
    "git blame",
    "git ls-files",
    "git rev-parse",
    "git grep",
    "git describe",
    "git shortlog",
    "git cat-file",
    "git ls-tree",
    "git rev-list",
    "git reflog",
    "brew list",
    "brew info",
];
const SUBCOMMAND_COMMANDS: &[&str] = &[
    "git",
    "cargo",
    "uv",
    "docker",
    "npm",
    "pnpm",
    "yarn",
    "dotnet",
    "brew",
    "systemctl",
    "gh",
    "tmux",
    "pip",
    "pip3",
    "go",
    "kubectl",
    "launchctl",
    "apt",
    "apt-get",
    "rustup",
    "just",
];
const WRAPPER_COMMANDS: &[&str] = &[
    "sudo", "time", "nohup", "command", "exec", "env", "nice", "builtin",
];
const PREFIX_KEYWORDS: &[&str] = &[
    "do", "then", "else", "{", "!", "if", "elif", "while", "until",
];
const SKIPPED_KEYWORDS: &[&str] = &["for", "case", "select", "done", "fi", "esac", "}", "in"];
const SHELLS: &[&str] = &["bash", "sh", "zsh"];
const INLINE_FLAG: &str = "-c";
const TIMEOUT_COMMAND: &str = "timeout";
const SED_COMMAND: &str = "sed";
const TAR_COMMAND: &str = "tar";
const TAR_LISTING_FLAG: char = 't';
const TAR_WRITING_FLAGS: &[char] = &['x', 'c', 'r', 'u'];
const IN_PLACE_FLAGS: &[&str] = &["-i", "--in-place"];
const REDIRECT_LABELS: [&str; 2] = [">", ">>"];
const DEV_NULL_PREFIX: &str = "/dev/";

#[derive(Debug, PartialEq, Eq)]
pub struct Invocation {
    pub label: String,
    pub text: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    pub effects: Vec<Invocation>,
    pub inspections: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    Boundary,
    Redirect { append: bool, ignored: bool },
    InputRedirect,
}

#[derive(Default)]
struct Segment {
    words: Vec<String>,
    writes: Vec<(bool, String)>,
}

pub fn analyze(command: &str) -> Analysis {
    let mut analysis = Analysis::default();
    for segment in segments(&lex(command)) {
        for (append, target) in &segment.writes {
            analysis.effects.push(Invocation {
                label: REDIRECT_LABELS[usize::from(*append)].to_owned(),
                text: format!("{} {target}", REDIRECT_LABELS[usize::from(*append)]),
            });
        }
        classify(&segment.words, &mut analysis);
    }
    analysis
}

fn classify(words: &[String], analysis: &mut Analysis) {
    let mut words = words;
    while let Some(first) = words.first() {
        let is_assignment = first.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        });
        if is_assignment
            || PREFIX_KEYWORDS.contains(&first.as_str())
            || WRAPPER_COMMANDS.contains(&first.as_str())
        {
            words = &words[1..];
        } else if first == TIMEOUT_COMMAND {
            words = words.get(2..).unwrap_or_default();
        } else {
            break;
        }
    }
    let Some(head) = words.first() else {
        return;
    };
    if SKIPPED_KEYWORDS.contains(&head.as_str()) {
        return;
    }
    if SHELLS.contains(&head.as_str())
        && let Some(position) = words.iter().position(|word| word == INLINE_FLAG)
        && let Some(inner) = words.get(position + 1)
    {
        let nested = analyze(inner);
        analysis.effects.extend(nested.effects);
        analysis.inspections += nested.inspections;
        return;
    }

    let label = label_of(words);
    let edits_in_place = head == SED_COMMAND
        && words
            .iter()
            .any(|word| IN_PLACE_FLAGS.contains(&word.as_str()));
    let only_reads = READ_ONLY_COMMANDS.contains(&label.as_str()) && !edits_in_place;
    if only_reads || lists_archive(words) {
        analysis.inspections += 1;
    } else {
        analysis.effects.push(Invocation {
            label: if edits_in_place {
                format!("{SED_COMMAND} -i")
            } else {
                label
            },
            text: words.join(" "),
        });
    }
}

/// Only the mode word and dashed flags can hold tar's mode letters; `tar -I zstd -tf x`
/// has a plain `zstd` argument that must not be read as flags.
fn lists_archive(words: &[String]) -> bool {
    words[0] == TAR_COMMAND
        && words[1..]
            .iter()
            .enumerate()
            .filter(|(index, word)| *index == 0 || word.starts_with('-'))
            .any(|(_, word)| {
                let flags = word.trim_start_matches('-');
                !flags.is_empty()
                    && flags.chars().all(|c| c.is_ascii_alphabetic())
                    && flags.contains(TAR_LISTING_FLAG)
                    && !flags.contains(TAR_WRITING_FLAGS)
            })
}

fn label_of(words: &[String]) -> String {
    let head = words[0].rsplit('/').next().unwrap_or(&words[0]);
    let subcommand = SUBCOMMAND_COMMANDS
        .contains(&head)
        .then(|| words[1..].iter().find(|word| !word.starts_with('-')))
        .flatten();
    match subcommand {
        Some(subcommand) => format!("{head} {subcommand}"),
        None => head.to_owned(),
    }
}

fn segments(tokens: &[Token]) -> Vec<Segment> {
    let mut segments = vec![Segment::default()];
    let mut tokens = tokens.iter();
    while let Some(token) = tokens.next() {
        let current = segments.last_mut().expect("segments starts non-empty");
        match token {
            Token::Word(word) => current.words.push(word.clone()),
            Token::Boundary => segments.push(Segment::default()),
            Token::InputRedirect => {
                tokens.next();
            }
            Token::Redirect { append, ignored } => {
                if let Some(Token::Word(target)) = tokens.next()
                    && !ignored
                    && !target.starts_with(DEV_NULL_PREFIX)
                {
                    current.writes.push((*append, target.clone()));
                }
            }
        }
    }
    segments
}

struct HeredocTag {
    tag: String,
    strip_tabs: bool,
}

fn lex(command: &str) -> Vec<Token> {
    let chars: Vec<char> = command.chars().collect();
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut pending_heredocs: Vec<HeredocTag> = Vec::new();
    let mut i = 0;

    macro_rules! flush {
        () => {
            if in_word {
                tokens.push(Token::Word(std::mem::take(&mut word)));
                in_word = false;
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => {
                flush!();
                i += 1;
            }
            '\n' => {
                flush!();
                tokens.push(Token::Boundary);
                i += 1;
                for heredoc in pending_heredocs.drain(..) {
                    i = skip_heredoc_body(&chars, i, &heredoc);
                }
            }
            '#' if !in_word => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '\'' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
                i += 1;
            }
            '"' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
                i += 1;
            }
            '\\' => {
                if chars.get(i + 1).is_some_and(|next| *next != '\n') {
                    in_word = true;
                    word.push(chars[i + 1]);
                }
                i += 2;
            }
            '$' if chars.get(i + 1) == Some(&'(') => {
                in_word = true;
                let end = matching_paren(&chars, i + 1);
                word.extend(&chars[i..end.min(chars.len())]);
                i = end;
            }
            '`' => {
                in_word = true;
                let end = chars[i + 1..]
                    .iter()
                    .position(|c| *c == '`')
                    .map_or(chars.len(), |p| i + p + 2);
                word.extend(&chars[i..end.min(chars.len())]);
                i = end;
            }
            ';' | '|' | '&' if !(c == '&' && chars.get(i + 1) == Some(&'>')) => {
                flush!();
                tokens.push(Token::Boundary);
                i += 1;
                while matches!(chars.get(i), Some('&' | '|')) {
                    i += 1;
                }
            }
            '<' if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) != Some(&'<') => {
                flush!();
                i += 2;
                let strip_tabs = chars.get(i) == Some(&'-');
                i += usize::from(strip_tabs);
                while matches!(chars.get(i), Some(' ' | '\t')) {
                    i += 1;
                }
                let mut tag = String::new();
                while let Some(tag_char) = chars
                    .get(i)
                    .filter(|c| !c.is_whitespace() && !matches!(c, ';' | '|' | '&' | ')'))
                {
                    if !matches!(tag_char, '\'' | '"' | '\\') {
                        tag.push(*tag_char);
                    }
                    i += 1;
                }
                pending_heredocs.push(HeredocTag { tag, strip_tabs });
            }
            '<' => {
                flush!();
                tokens.push(Token::InputRedirect);
                i += 1 + usize::from(chars.get(i + 1) == Some(&'<')) * 2;
            }
            '>' | '&' => {
                let is_fd_prefix = in_word && word.chars().all(|d| d.is_ascii_digit());
                if is_fd_prefix {
                    word.clear();
                    in_word = false;
                } else {
                    flush!();
                }
                i += usize::from(c == '&');
                let append = chars.get(i + 1) == Some(&'>');
                i += 1 + usize::from(append);
                let to_descriptor = chars.get(i) == Some(&'&');
                i += usize::from(to_descriptor || chars.get(i) == Some(&'|'));
                tokens.push(Token::Redirect {
                    append,
                    ignored: to_descriptor,
                });
            }
            _ => {
                in_word = true;
                word.push(c);
                i += 1;
            }
        }
    }
    if in_word {
        tokens.push(Token::Word(word));
    }
    tokens
}

fn matching_paren(chars: &[char], open: usize) -> usize {
    let mut depth = 0;
    for (offset, c) in chars[open..].iter().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return open + offset + 1;
                }
            }
            _ => {}
        }
    }
    chars.len()
}

fn skip_heredoc_body(chars: &[char], mut i: usize, heredoc: &HeredocTag) -> usize {
    while i < chars.len() {
        let end = chars[i..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(chars.len(), |p| i + p);
        let line: String = chars[i..end].iter().collect();
        i = (end + 1).min(chars.len());
        let line = if heredoc.strip_tabs {
            line.trim_start_matches('\t')
        } else {
            &line
        };
        if line == heredoc.tag {
            break;
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    fn labels(command: &str) -> Vec<String> {
        analyze(command)
            .effects
            .into_iter()
            .map(|effect| effect.label)
            .collect()
    }

    #[test_case("ls -la && cat a.txt | grep x", &[]; "pure_inspection")]
    #[test_case("git status && git commit -m 'msg; with && ops'", &["git commit"]; "quoted_operators_do_not_split")]
    #[test_case("cd /tmp && cargo test -p foo 2>&1 | tail -5", &["cargo test"]; "cd_and_pipes")]
    #[test_case("FOO=1 RUST_LOG=x timeout 30 cargo build", &["cargo build"]; "env_and_timeout_prefixes")]
    #[test_case("sed -n '1,5p' f && sed -i 's/a/b/' f", &["sed -i"]; "sed_only_in_place_is_an_effect")]
    #[test_case("echo hi > out.txt", &[">"]; "redirect_to_file")]
    #[test_case("make 2>&1 >/dev/null; echo done >> log", &["make", ">>"]; "dev_null_and_fd_dups_are_not_writes")]
    #[test_case("for f in a b; do rm $f; done", &["rm"]; "loop_bodies_are_commands")]
    #[test_case("bash -c 'git push && ls'", &["git push"]; "inline_shell_is_unwrapped")]
    #[test_case("git log --oneline | head", &[]; "read_only_git")]
    #[test_case("# git push\nls", &[]; "comments_are_ignored")]
    #[test_case("tar -I zstd -tf a.tar.zst", &[]; "tar_listing_with_a_plain_word_argument")]
    #[test_case("tar tf a.tar", &[]; "tar_listing_mode_word")]
    #[test_case("tar xzf a.tgz && tar -czf b.tgz dir", &["tar", "tar"]; "tar_extract_and_create_are_effects")]
    #[test_case("brew list zstd", &[]; "brew_list")]
    fn classifies_commands(command: &str, expected: &[&str]) {
        assert_eq!(labels(command), expected);
    }

    #[test]
    fn heredoc_bodies_are_not_commands() {
        let command = "cat > main.rs <<'EOF'\nfn main() {\n  rm -rf /\n}\nEOF\ncargo build";
        assert_eq!(labels(command), [">", "cargo build"]);
    }

    #[test]
    fn effects_keep_the_literal_command_text() {
        let analysis = analyze("git commit -m \"fix it\"");
        assert_eq!(analysis.effects[0].text, "git commit -m fix it");
    }

    #[test]
    fn inspections_are_counted_not_listed() {
        assert_eq!(analyze("ls && cat a | wc -l").inspections, 3);
    }

    #[test]
    fn command_substitution_stays_inside_its_word() {
        assert_eq!(labels("echo $(rm -rf x; ls) && cargo fmt"), ["cargo fmt"]);
    }
}
