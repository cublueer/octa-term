//! 数学表达式识别与完整性扫描。
//!
//! 两个职责：
//! - `looks_like_math`：输入是否「像」Octave 表达式（启发式，宁缺毋滥——
//!   不像的交给 shell 报错即可）
//! - `expr_is_complete`：括号/字符串是否闭合（未闭合 → 钩子自动续行）
//!
//! 消歧难点集中在「标识符后跟运算符」：`x-y`（变量减法）与
//! `my-script.sh`（文件名）、`a.b`（结构体字段）与 `README.md`（文件）形态
//! 相同。取舍原则：**数学优先，只有带扩展名结尾（`.ext` 到行尾）才认作
//! 文件**；纯路径形态（`my-app/bin`）无法与 `a-b/c` 区分，归数学，由
//! Octave 报错兜底（文档说明）。

use super::token::is_cjk_char;
use crate::octave_funcs::is_octave_function;

pub fn contains_cjk(input: &str) -> bool {
    input.chars().any(is_cjk_char)
}

/// 主判定：命令闸门已放行（不是命令、不含 CJK）后的数学启发式。
pub fn looks_like_math(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }

    // 数字 / 点 / 括号 / 符号开头：直接按数学
    if let Some(first) = trimmed.chars().next() {
        if first.is_ascii_digit() || matches!(first, '.' | '(' | '[' | '+' | '-' | '\'' | '"') {
            return true;
        }
    }

    // 标识符开头：裸标识符 / 函数调用 / 赋值 / 运算符
    if let Some((ident, rest)) = leading_identifier_and_rest(trimmed) {
        let rest = rest.trim_start();
        if rest.is_empty() {
            // 裸标识符：常量、函数名、或 daemon 模式下的变量求值
            return true;
        }
        let next = rest.chars().next().unwrap();
        return match next {
            '(' | '\'' => true, // 函数调用 / 转置
            // `det [1 2; 3 4]`：函数名后直接跟矩阵（合法 Octave 语法），
            // 靠函数名表识别；普通词后跟 `[` 是自然语言
            '[' => is_octave_function(ident),
            '*' | '/' | '^' | '\\' | '=' | '<' | '>' | '~' | '&' | '|' | ':' => true,
            '+' | '-' | '.' => identifier_follower_is_math(rest),
            _ => false, // `sin is nice` 之类自然语言
        };
    }

    false
}

/// 取开头的最长标识符与其后剩余部分（供子 shell 内层首词判定复用）。
pub(crate) fn leading_identifier_and_rest(input: &str) -> Option<(&str, &str)> {
    let bytes = input.as_bytes();
    let first = *bytes.first()?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    let mut end = 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    Some((&input[..end], &input[end..]))
}

/// 标识符后跟 `+`/`-`/`.` 的消歧（`rest[0]` 就是该符号）。
///
/// - `a-2`、`a-(b)`、`a.'` → 数学
/// - `x-y+1` → 后面还有运算符 → 数学
/// - `my-script.sh`、`README.md` → 词段以 `.ext` 结尾且无后续运算符 → 文件
/// - `x-y`、`total-cost` → 无扩展名的连字符词 → 数学（变量减法优先）
fn identifier_follower_is_math(rest: &str) -> bool {
    let sign = rest.chars().next().unwrap();
    let after = &rest[1..];
    let trimmed = after.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return false; // `a-` 后无内容
    };
    if first.is_ascii_digit() || matches!(first, '(' | '[' | '\'' | '"') {
        return true;
    }
    // 词段字符：`.` 分支只吃标识符字符；`+`/`-` 分支还吃连字符与点
    let word_char = |c: char| {
        c.is_ascii_alphanumeric() || c == '_' || (sign != '.' && (c == '-' || c == '.'))
    };
    let word_end = trimmed
        .find(|c: char| !word_char(c))
        .unwrap_or(trimmed.len());
    let word = &trimmed[..word_end];
    let remainder = trimmed[word_end..].trim_start();
    if !remainder.is_empty() {
        return true; // 词段后还有运算符/括号 → 表达式
    }
    if sign == '.' {
        return false; // `ident.ident` 到行尾 → 疑似文件名
    }
    !word.contains('.') // `x-y` 数学；`my-script.sh` 文件
}

/// 括号/字符串闭合扫描。跳过 `'...'` 字符串（含 `''` 转义）、`"..."` 字符串
/// （含 `\` 转义）、`%` 行注释、`#{ ... #}` 块注释。
///
/// `'` 在 Octave 里既是字符串定界又是转置：只有出现在「开串位」才按字符串
/// 处理，`[1 2]'` 里的 `'` 是转置、不参与配对。
pub fn expr_is_complete(input: &str) -> bool {
    let mut stack: Vec<char> = Vec::new();
    let mut state = ScanState::Normal;
    let mut prev_significant: Option<char> = None;

    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match state {
            ScanState::Normal => match ch {
                '%' => state = ScanState::Comment,
                '#' if chars.peek() == Some(&'{') => {
                    chars.next();
                    state = ScanState::BlockComment;
                }
                '\'' if opens_string(prev_significant) => state = ScanState::SQuote,
                '\'' => prev_significant = Some('\''), // 转置
                '"' => state = ScanState::DQuote,
                '(' | '[' | '{' => {
                    stack.push(ch);
                    prev_significant = Some(ch);
                }
                ')' | ']' | '}' => {
                    let expect = match ch {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    match stack.pop() {
                        // 配对成功继续；错配/多余闭合不再续行，交给 Octave 报错
                        Some(open) if open == expect => {}
                        _ => return true,
                    }
                    prev_significant = Some(ch);
                }
                c if !c.is_whitespace() => prev_significant = Some(c),
                _ => {}
            },
            ScanState::SQuote => {
                if ch == '\'' {
                    state = ScanState::Normal;
                    prev_significant = Some('\'');
                }
            }
            ScanState::DQuote => match ch {
                '"' => {
                    state = ScanState::Normal;
                    prev_significant = Some('"');
                }
                '\\' => state = ScanState::DEscaped,
                _ => {}
            },
            ScanState::DEscaped => state = ScanState::DQuote,
            ScanState::Comment => {
                if ch == '\n' {
                    state = ScanState::Normal;
                }
            }
            ScanState::BlockComment => {
                // Octave 块注释以 `#}` 闭合（`#` 在前）
                if ch == '#' && chars.peek() == Some(&'}') {
                    chars.next();
                    state = ScanState::Normal;
                }
            }
        }
    }

    // 行尾 `%` 注释是合法的；字符串/块注释未闭合才算不完整
    stack.is_empty() && matches!(state, ScanState::Normal | ScanState::Comment)
}

#[derive(PartialEq)]
enum ScanState {
    Normal,
    SQuote,
    DQuote,
    DEscaped,
    Comment,
    BlockComment,
}

/// `'` 出现在这些位置之后时按字符串开始处理，否则按转置。
fn opens_string(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(ch) => matches!(ch, '(' | ',' | '=' | '[' | '{' | ':' | ';'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_math_starts() {
        assert!(looks_like_math("1+1"));
        assert!(looks_like_math("(1+2)*3"));
        assert!(looks_like_math("[1 2; 3 4]"));
        assert!(looks_like_math("sin(pi/2)"));
        assert!(looks_like_math("det([1 2; 3 4])"));
        assert!(looks_like_math("pi"));
        assert!(looks_like_math("x = 5"));
        assert!(looks_like_math("a'"));
        assert!(looks_like_math("[1 2]'"));
        assert!(looks_like_math("x-y"));
        assert!(looks_like_math("a-2"));
        assert!(looks_like_math("'abc'"));
        assert!(looks_like_math("a.b+1"));
        assert!(looks_like_math("a.*b"));
        assert!(looks_like_math("a-b/c"));
        assert!(looks_like_math("a--2"));
        // 函数名后直接跟矩阵是合法 Octave 语法（靠函数名表识别）
        assert!(looks_like_math("det [1 2; 3 4]"));
    }

    #[test]
    fn rejects_non_math() {
        assert!(!looks_like_math("hello world"));
        assert!(!looks_like_math("sin is nice"));
        assert!(!looks_like_math("my-script.sh"));
        assert!(!looks_like_math("some-file.tar.gz"));
        assert!(!looks_like_math("README.md"));
        assert!(!looks_like_math("a.b"));
        assert!(!looks_like_math(""));
        assert!(!looks_like_math("你好"));
        // 普通词后跟 `[` 是自然语言，不是数学
        assert!(!looks_like_math("foo [bar]"));
    }

    #[test]
    fn completeness_brackets() {
        assert!(expr_is_complete("1+1"));
        assert!(expr_is_complete("det([1 2; 3 4])"));
        assert!(expr_is_complete("(1+2)*(3+4)"));
        assert!(!expr_is_complete("det([1 2;"));
        assert!(!expr_is_complete("(1+2"));
        assert!(!expr_is_complete("sqrt(2"));
    }

    #[test]
    fn completeness_ignores_strings_and_comments() {
        assert!(expr_is_complete("disp('(not a bracket')"));
        assert!(expr_is_complete("disp(\"(not a bracket\")"));
        assert!(expr_is_complete("1+1 % (comment"));
        assert!(expr_is_complete("#{ (block } not bracket #} 1+1"));
        assert!(!expr_is_complete("disp('unterminated"));
        assert!(!expr_is_complete("disp(\"unterminated"));
    }

    #[test]
    fn transpose_vs_string() {
        assert!(expr_is_complete("[1 2]'"));
        assert!(expr_is_complete("a'"));
        assert!(!expr_is_complete("'abc"));
        assert!(expr_is_complete("'abc'"));
    }

    #[test]
    fn multiline_completeness() {
        assert!(expr_is_complete("[1 2;\n3 4]"));
        assert!(!expr_is_complete("[1 2;\n3 4"));
    }
}
