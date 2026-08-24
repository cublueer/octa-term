//! 表达式安全过滤：进入 Octave 前的最后一道闸。
//!
//! Octave 有能力执行外部命令（`system("...")`、`!cmd` 外壳转义）与文件
//! 操作，而钩子拦截的是用户在终端里随便打的内容，所以必须两道防线：
//!
//! 1. **字符集**：只允许数学字符，排除 `!`（外壳转义）、反引号、`$`、
//!    控制字符与非 ASCII——从源头杜绝「整行原样执行」类攻击；
//! 2. **函数黑名单**：扫描标识符，命中危险内建（system/unix/eval/feval/
//!    input/pause…）即拒绝，堵住 `str2func`/`feval` 字符串绕行。

/// 危险内建（小写比较）。宁可误伤变量名，安全优先。
///
/// 除了执行类（system/eval/feval…），还拦破坏性文件操作（delete/unlink/
/// rename/save…）——钩子拦下的是「看起来像公式」的输入，文件操作应当
/// 走 shell 而不是 Octave 表达式。
const BANNED_FUNCTIONS: &[&str] = &[
    "assignin", "copyfile", "csvread", "csvwrite", "dbclear", "dbquit",
    "dbstack", "dbstatus", "dbstop", "dbup", "delete", "dlmread", "dlmwrite",
    "dos", "eval", "evalc", "evalin", "feval", "fopen", "inline", "input",
    "keyboard", "load", "mex", "mkoctfile", "movefile", "pause", "perl",
    "pkg", "popen", "popen2", "python", "rename", "rmdir", "run", "save",
    "source", "str2func", "system", "unix", "unlink",
];

/// 字符集校验与黑名单扫描。命中时返回人类可读的原因。
pub fn check(expr: &str) -> Result<(), String> {
    check_charset(expr)?;
    check_banned_functions(expr)
}

fn check_charset(expr: &str) -> Result<(), String> {
    for ch in expr.chars() {
        let ok = ch.is_ascii_alphanumeric()
            || ch.is_ascii_whitespace()
            || matches!(
                ch,
                '.' | '_' | '+' | '-' | '*' | '/' | '\\' | '^' | '(' | ')'
                    | '[' | ']' | '{' | '}' | ';' | ',' | '\'' | '"' | ':'
                    | '~' | '<' | '>' | '=' | '&' | '|' | '%' | '@' | '#'
            );
        if !ok {
            return Err(format!("包含不允许的字符 {ch:?}"));
        }
    }
    Ok(())
}

fn check_banned_functions(expr: &str) -> Result<(), String> {
    let mut ident = String::new();
    let flush = |ident: &mut String| -> Result<(), String> {
        if !ident.is_empty() {
            let lower = ident.to_ascii_lowercase();
            if BANNED_FUNCTIONS.binary_search(&lower.as_str()).is_ok() {
                return Err(format!("函数 {ident} 被安全策略拦截"));
            }
            ident.clear();
        }
        Ok(())
    };
    for ch in expr.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            ident.push(ch);
        } else {
            flush(&mut ident)?;
        }
    }
    flush(&mut ident)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_normal_math() {
        assert!(check("1+1").is_ok());
        assert!(check("det([1 2; 3 4])").is_ok());
        assert!(check("x = sin(pi/2)").is_ok());
        assert!(check("'hello'").is_ok());
        assert!(check("format long; 1/3").is_ok());
    }

    #[test]
    fn blocks_banned_functions() {
        for name in [
            "system", "System", "unix", "eval", "feval", "input", "pause",
            "fopen", "delete", "unlink", "save", "load", "movefile",
        ] {
            assert!(check(&format!("{name}('x')")).is_err(), "should block {name}");
        }
    }

    #[test]
    fn blocks_shell_escape_and_control_chars() {
        assert!(check("!echo hi").is_err());
        assert!(check("1+1; system('id')").is_err());
        assert!(check("a`b").is_err());
        assert!(check("$HOME").is_err());
        assert!(check("你好").is_err());
    }

    #[test]
    fn allows_harmless_similar_names() {
        // 黑名单按整词匹配：systematic 不是 system
        assert!(check("x = 1; systematic").is_ok());
        assert!(check("x=1").is_ok());
    }
}
