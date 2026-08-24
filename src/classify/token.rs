//! 命令闸门：判定输入是否应交给 shell 执行。
//!
//! 思路沿用 Miyu 的成熟做法：取「第一个命令词」（跳过环境变量赋值，处理
//! 引号/转义/注释，`;|&<>` 截断），然后三层判定——shell 关键字/内建、
//! 显式路径、PATH 可执行。对 `time`/`test` 这类既是命令又可能是自然语言
//! 开头的词做歧义护栏。

use std::env;
use std::fs;

/// 输入是否是一条 shell 命令。
pub fn is_shell_command(input: &str, shell_name: &str) -> bool {
    let Some((command, rest)) = first_command_token_with_rest(input) else {
        return false;
    };
    if ambiguous_command_tail_looks_like_message(&command, rest) {
        return false;
    }
    is_command_word(&command, shell_name)
}

/// 单个词是否是命令：关键字/内建、显式路径或 PATH 可执行。
/// 供子 shell（`(cd /tmp && ls)`）的内层首词判定复用。
pub fn is_command_word(command: &str, shell_name: &str) -> bool {
    is_shell_keyword_or_builtin(command, shell_name)
        || is_explicit_command_path(command)
        || command_exists_in_path(command)
}

fn first_command_token_with_rest(input: &str) -> Option<(String, &str)> {
    let mut offset = 0;
    while let Some(token) = next_fish_like_token(input, &mut offset) {
        if is_env_assignment(&token) {
            continue;
        }
        return Some((token, input.get(offset..).unwrap_or("")));
    }
    None
}

/// `time`/`test`/`date` 等词后面跟问号或中文时，更像提问而不是命令。
fn ambiguous_command_tail_looks_like_message(command: &str, rest: &str) -> bool {
    if !matches!(
        command,
        "time" | "test" | "date" | "which" | "type" | "command" | "history"
    ) {
        return false;
    }
    let rest = rest.trim();
    !rest.is_empty()
        && rest
            .chars()
            .any(|ch| ch == '?' || ch == '？' || is_cjk_char(ch))
}

pub fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2A6DF}'
            | '\u{2A700}'..='\u{2B73F}'
            | '\u{2B740}'..='\u{2B81F}'
            | '\u{2B820}'..='\u{2CEAF}'
    )
}

/// fish 风格分词：跳过空白与 `#` 注释，处理单双引号与转义，
/// 在 `;|&<>` 处截断。
fn next_fish_like_token(input: &str, offset: &mut usize) -> Option<String> {
    let mut index = *offset;
    loop {
        let rest = input.get(index..)?;
        let Some(ch) = rest.chars().next() else {
            *offset = input.len();
            return None;
        };
        if ch.is_whitespace() {
            index += ch.len_utf8();
            continue;
        }
        if ch == '#' {
            index += ch.len_utf8();
            while let Some(next) = input.get(index..).and_then(|rest| rest.chars().next()) {
                index += next.len_utf8();
                if next == '\n' || next == '\r' {
                    break;
                }
            }
            continue;
        }
        break;
    }

    let mut token = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut consumed = input.len();

    for (relative, ch) in input[index..].char_indices() {
        let absolute = index + relative;
        if escaped {
            token.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if !in_single
            && !in_double
            && (ch.is_whitespace() || matches!(ch, ';' | '|' | '&' | '<' | '>'))
        {
            consumed = absolute + ch.len_utf8();
            if token.is_empty() {
                token.push(ch);
            }
            break;
        }
        token.push(ch);
    }

    *offset = consumed;
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_shell_keyword_or_builtin(command: &str, shell_name: &str) -> bool {
    let common = matches!(
        command,
        "["
            | "[["
            | "alias"
            | "bg"
            | "break"
            | "builtin"
            | "case"
            | "cd"
            | "command"
            | "continue"
            | "else"
            | "end"
            | "exec"
            | "exit"
            | "false"
            | "fg"
            | "for"
            | "function"
            | "functions"
            | "history"
            | "if"
            | "jobs"
            | "not"
            | "or"
            | "and"
            | "read"
            | "return"
            | "set"
            | "source"
            | "status"
            | "switch"
            | "test"
            | "time"
            | "true"
            | "while"
    );
    common
        || (shell_name == "fish"
            && matches!(
                command,
                "abbr"
                    | "argparse"
                    | "begin"
                    | "bind"
                    | "block"
                    | "contains"
                    | "count"
                    | "disown"
                    | "emit"
                    | "eval"
                    | "math"
                    | "random"
                    | "string"
                    | "type"
                    | "ulimit"
            ))
}

fn is_explicit_command_path(command: &str) -> bool {
    command.starts_with('/')
        || command.starts_with("./")
        || command.starts_with("../")
        || command.starts_with("~/")
}

fn command_exists_in_path(command: &str) -> bool {
    if command.is_empty() || command.contains('/') {
        return false;
    }
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&paths).any(|dir| is_executable_file(&dir.join(command)))
}

fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_commands() {
        assert!(is_shell_command("echo hi", "fish"));
        assert!(is_shell_command("cd /tmp", "fish"));
        assert!(is_shell_command("FOO=bar cargo check", "fish"));
        assert!(is_shell_command("# comment\nls", "fish"));
        assert!(is_shell_command("./target/debug/octa-term hi", "fish"));
        assert!(is_shell_command("for item in a b", "fish"));
        assert!(is_shell_command("time cargo check", "fish"));
        assert!(is_shell_command("git status", "fish"));
    }

    #[test]
    fn rejects_non_commands() {
        assert!(!is_shell_command("你觉得 a;b 是什么意思", "fish"));
        assert!(!is_shell_command("解释 <tag> 是什么", "fish"));
        assert!(!is_shell_command("第一行\n第二行", "fish"));
        assert!(!is_shell_command("# note\n解释一下这个问题", "fish"));
        assert!(!is_shell_command("time 是什么命令？", "fish"));
        assert!(!is_shell_command("this-command-does-not-exist", "fish"));
        assert!(!is_shell_command("1+1", "fish"));
        assert!(!is_shell_command("sin(pi)", "fish"));
    }
}
