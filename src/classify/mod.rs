//! 四态分类器：把终端输入分流为「shell 命令 / 数学表达式 / 未闭合续行 /
//! 其他」。这是 octa-term 的核心，也是 shell 钩子的契约（退出码 0/1/2/3）。

pub mod math;
pub mod safety;
pub mod token;

/// 分类结论，对应 `--shell-classify` 的退出码：
/// Command=0、Math=1、Incomplete=2、Other=3。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 交给 shell 正常执行
    Command,
    /// 交给 Octave 求值
    Math,
    /// 括号/字符串未闭合：shell 钩子应插入换行让用户继续输入
    Incomplete,
    /// 都不是：放行给 shell，让它自己报 command not found / 语法错误
    Other,
}

impl Verdict {
    pub fn exit_code(self) -> i32 {
        match self {
            Verdict::Command => 0,
            Verdict::Math => 1,
            Verdict::Incomplete => 2,
            Verdict::Other => 3,
        }
    }
}

/// 分类主入口。
///
/// 顺序即安全优先级：命令闸门在前（任何像命令的输入绝不进 Octave），
/// 自然语言（CJK）在后，剩下的才可能是数学。
pub fn classify(input: &str, shell: &str) -> Verdict {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Verdict::Other;
    }

    // `=` 前缀：强制按表达式求值，跳过命令闸门（仍受完整性/安全检查）。
    if let Some(forced) = trimmed.strip_prefix('=') {
        let forced = forced.trim();
        if forced.is_empty() {
            return Verdict::Other;
        }
        return if math::expr_is_complete(forced) {
            Verdict::Math
        } else {
            Verdict::Incomplete
        };
    }

    // 命令闸门：像命令就执行，绝不拦截。
    if token::is_shell_command(input, shell) {
        return Verdict::Command;
    }

    // 子 shell 惯用法：`(cd /tmp && ls)` 的内层首词是命令时，整条交给
    // shell（否则会被 Octave 吃掉：`cd` 是 octave 内建，会真的改 octave
    // 的 cwd）。fish 的 `((1+2))` 数学内建同理。
    if trimmed.starts_with("((") {
        return Verdict::Command;
    }
    if let Some(inner) = trimmed.strip_prefix('(') {
        if let Some((first, _)) = math::leading_identifier_and_rest(inner.trim_start()) {
            if token::is_command_word(first, shell) {
                return Verdict::Command;
            }
        }
    }

    // 含 CJK：是自然语言，不是数学也不是命令。
    if math::contains_cjk(input) {
        return Verdict::Other;
    }

    if math::looks_like_math(input) {
        // Octave 行续接 `...`：交给钩子自动续行（`1 + ...` ↵ 后继续输）；
        // 裸 `...` 在终端语境没有意义，放行给 shell 报错。
        if trimmed == "..." {
            return Verdict::Other;
        }
        if trimmed.ends_with("...") {
            return Verdict::Incomplete;
        }
        return if math::expr_is_complete(input) {
            Verdict::Math
        } else {
            Verdict::Incomplete
        };
    }

    Verdict::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(input: &str) -> Verdict {
        classify(input, "fish")
    }

    #[test]
    fn basic_arithmetic_is_math() {
        assert_eq!(verdict("1+1"), Verdict::Math);
        assert_eq!(verdict("2^3"), Verdict::Math);
        assert_eq!(verdict("0.5*3"), Verdict::Math);
        assert_eq!(verdict("(1+2)*3"), Verdict::Math);
        assert_eq!(verdict(" -5"), Verdict::Math);
    }

    #[test]
    fn function_calls_are_math() {
        assert_eq!(verdict("sin(pi/2)"), Verdict::Math);
        assert_eq!(verdict("det([1 2; 3 4])"), Verdict::Math);
        assert_eq!(verdict("sqrt(2"), Verdict::Incomplete);
        assert_eq!(verdict("sum([1 2 3]"), Verdict::Incomplete);
    }

    #[test]
    fn constants_and_bare_identifiers_are_math() {
        assert_eq!(verdict("pi"), Verdict::Math);
        assert_eq!(verdict("ans"), Verdict::Math);
        assert_eq!(verdict("i"), Verdict::Math);
        // daemon 模式下变量求值：裸标识符算数学（Octave 会报未定义，可接受）
        assert_eq!(verdict("x"), Verdict::Math);
    }

    #[test]
    fn assignments_are_math() {
        assert_eq!(verdict("x = 5"), Verdict::Math);
        assert_eq!(verdict("a=det([1 2;3 4])"), Verdict::Math);
        assert_eq!(verdict("a = [1 2;"), Verdict::Incomplete);
    }

    #[test]
    fn shell_commands_pass_through() {
        assert_eq!(verdict("ls"), Verdict::Command);
        assert_eq!(verdict("cd /tmp"), Verdict::Command);
        assert_eq!(verdict("git status"), Verdict::Command);
        assert_eq!(verdict("echo [1"), Verdict::Command);
        assert_eq!(verdict("FOO=bar cargo check"), Verdict::Command);
        assert_eq!(verdict("./build.sh"), Verdict::Command);
        // 子 shell 惯用法绝不能进 Octave（octave 的 cd 会真改 cwd）
        assert_eq!(verdict("(cd /tmp && ls)"), Verdict::Command);
        assert_eq!(verdict("(git status)"), Verdict::Command);
        assert_eq!(verdict("(echo hi)"), Verdict::Command);
        // fish 数学内建 / bash 算术
        assert_eq!(verdict("((1+2))"), Verdict::Command);
        // test 内建
        assert_eq!(verdict("[ -f /etc/hosts ]"), Verdict::Command);
        assert_eq!(verdict("[[ -z $x ]]"), Verdict::Command);
    }

    #[test]
    fn octave_line_continuation_flows_to_hooks() {
        // `...` 续行交给钩子自动续行；裸 `...` 无意义交给 shell 报错
        assert_eq!(verdict("1 + 2 ..."), Verdict::Incomplete);
        assert_eq!(verdict("a = [1 2; ..."), Verdict::Incomplete);
        assert_eq!(verdict("..."), Verdict::Other);
    }

    #[test]
    fn natural_language_is_other() {
        assert_eq!(verdict("hello world"), Verdict::Other);
        assert_eq!(verdict("sin is nice"), Verdict::Other);
        assert_eq!(verdict("你好"), Verdict::Other);
        assert_eq!(verdict("time 是什么命令？"), Verdict::Other);
    }

    #[test]
    fn multiline_matrices_are_math() {
        assert_eq!(verdict("[1 2;\n3 4]"), Verdict::Math);
        assert_eq!(verdict("det([1 2;\n3 4])"), Verdict::Math);
        assert_eq!(verdict("[1 2;\n3 4"), Verdict::Incomplete);
    }

    #[test]
    fn transposes_and_strings() {
        assert_eq!(verdict("[1 2]'"), Verdict::Math);
        assert_eq!(verdict("'abc'"), Verdict::Math);
        assert_eq!(verdict("'abc"), Verdict::Incomplete);
    }

    #[test]
    fn force_prefix_bypasses_command_gate() {
        assert_eq!(verdict("=ls"), Verdict::Math);
        assert_eq!(verdict("=det([1 2;"), Verdict::Incomplete);
        assert_eq!(verdict("="), Verdict::Other);
    }

    #[test]
    fn dashed_words_are_not_math() {
        // 疑似带连字符的文件名，不给 Octave
        assert_eq!(verdict("my-script.sh"), Verdict::Other);
        assert_eq!(verdict("some-file.tar.gz"), Verdict::Other);
        // 但减号后跟数字/空格是正经减法
        assert_eq!(verdict("a-2"), Verdict::Math);
        assert_eq!(verdict("a- 2"), Verdict::Math);
    }

    #[test]
    fn variable_subtraction_with_identifiers_is_math() {
        assert_eq!(verdict("x-y"), Verdict::Math);
        assert_eq!(verdict("total-cost"), Verdict::Math);
    }

    #[test]
    fn empty_and_garbage() {
        assert_eq!(verdict(""), Verdict::Other);
        assert_eq!(verdict("   "), Verdict::Other);
    }
}
