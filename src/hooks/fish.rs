//! fish 钩子：全功能路线。
//!
//! - Enter 劫持：四态分类（0=命令执行 / 1=数学求值 / 2=未闭合续行 /
//!   3=放行让 fish 报错），数学表达式原样进历史、结果直接打印在当前行下方
//! - Ctrl+J：字面换行（多行矩阵）
//! - `fish_command_not_found`：单行未知命令兜底（Enter 绑定失效时仍可用）

use anyhow::Result;

use crate::paths::Paths;

pub fn hook() -> String {
    hook_with_bin(&super::hook_bin())
}

/// 用指定二进制路径生成钩子（回归测试注入不存在的路径用）。
pub fn hook_with_bin(bin: &str) -> String {
    format!(
        r#"# octa-term fish hook —— 自动生成，修改后请重新运行 `octa-term fish-init`
function __octa_accept_line
    status is-interactive; or return
    # 二进制不在 PATH（且不是可用绝对路径）时完全退化为原生行为，
    # 否则每次回车都会触发 command_not_found 递归
    if not type -q {bin}; and not test -x {bin}
        commandline -f execute
        return
    end
    set -l buffer (commandline -b | string collect)
    set -l trimmed (string trim -- "$buffer")
    if test -z "$trimmed"
        commandline -f execute
        return
    end
    # shell 语法未闭合（引号/括号等）→ 换行续打，不打扰
    commandline --is-valid
    if test $status -eq 2
        commandline -i \n
        commandline -f repaint
        return
    end
    # 四态分类：0=命令 1=数学 2=Octave 未闭合 3=其他
    printf '%s' "$buffer" | {bin} --shell-classify --shell fish --stdin 2>/dev/null
    set -l code $status
    switch $code
        case 1
            # 数学：原样进历史。Enter 被劫持后终端光标还停在输入行尾，
            # 先换到下一行，结果打印在表达式下方而不是挤在同一行
            printf '\n'
            history append -- "$buffer"
            printf '%s' "$buffer" | {bin} --shell-intercept --shell fish --stdin 2>/dev/null
            commandline -b -- ""
            commandline -f execute
        case 2
            # Octave 括号未闭合：自动续行
            commandline -i \n
            commandline -f repaint
        case '*'
            commandline -f execute
    end
end

function __octa_insert_newline
    commandline -i \n
end

bind enter __octa_accept_line
bind \r __octa_accept_line
bind -M insert enter __octa_accept_line
bind -M insert \r __octa_accept_line
bind ctrl-j __octa_insert_newline
bind \cj __octa_insert_newline
bind -M insert ctrl-j __octa_insert_newline
bind -M insert \cj __octa_insert_newline

function __octa_first_command
    set -l tokens (commandline --input="$argv[1]" --tokens-expanded 2>/dev/null)
    while test (count $tokens) -gt 0
        set -l token $tokens[1]
        if string match -qr '^[A-Za-z_][A-Za-z0-9_]*=' -- "$token"
            set -e tokens[1]
            continue
        end
        printf '%s' "$token"
        return 0
    end
    return 1
end

function fish_command_not_found
    status is-interactive; or return 127
    # 递归守卫：octa-term 自身找不到时绝不能再次调用它
    if not type -q {bin}; and not test -x {bin}
        printf 'fish: Unknown command: %s\n' "$argv[1]" >&2
        return 127
    end
    set -l current_line (status current-commandline 2>/dev/null | string collect)
    if test -n "$current_line"; and not string match -qr '[\n\r]' -- "$current_line"
        set -l top (__octa_first_command "$current_line")
        if test -z "$top"; or not type -q -- "$top"
            printf '%s' "$current_line" | {bin} --shell-intercept --shell fish --stdin 2>/dev/null
            # 不是数学：fish 有 handler 时不再自动打印错误，这里补回
            or printf 'fish: Unknown command: %s\n' "$top" >&2
            return 127
        end
    end
    set -l text (string join ' ' -- $argv)
    string match -qr '[\n\r]' -- "$text"; and return 127
    {bin} --shell-intercept --shell fish -- $text 2>/dev/null
    or printf 'fish: Unknown command: %s\n' "$argv[1]" >&2
    return 127
end
"#,
    )
}

pub fn install(paths: &Paths) -> Result<()> {
    if let Some(parent) = paths.fish_hook_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&paths.fish_hook_file, hook())?;
    println!("已安装 fish hook：{}", paths.fish_hook_file.display());
    super::warn_if_binary_missing_from_path();
    super::print_reload_hint("fish", &paths.fish_hook_file);
    Ok(())
}

pub fn uninstall(paths: &Paths) -> Result<bool> {
    let removed = match std::fs::remove_file(&paths.fish_hook_file) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(err.into()),
    };
    if removed {
        println!("已移除 fish hook");
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_defines_accept_line_binding() {
        let script = hook();
        assert!(script.contains("function __octa_accept_line"));
        assert!(script.contains("--shell-classify --shell fish --stdin"));
        assert!(script.contains("--shell-intercept --shell fish --stdin"));
        assert!(script.contains("bind enter __octa_accept_line"));
        assert!(script.contains("bind -M insert enter __octa_accept_line"));
    }

    #[test]
    fn hook_defines_newline_and_fallback() {
        let script = hook();
        assert!(script.contains("__octa_insert_newline"));
        assert!(script.contains("bind ctrl-j __octa_insert_newline"));
        assert!(script.contains("fish_command_not_found"));
        assert!(script.contains("return 127"));
        // 缺二进制守卫：二进制不可用时绝不调用，防递归
        assert!(script.contains("if not type -q "));
        assert!(script.contains("and not test -x "));
    }

    #[test]
    fn hook_handles_four_state_switch() {
        let script = hook();
        assert!(script.contains("case 1"));
        assert!(script.contains("case 2"));
        assert!(script.contains("case '*'"));
        assert!(script.contains("history append -- \"$buffer\""));
        // 结果前换行：Enter 被劫持后光标停在输入行尾，不换行会挤成一行
        assert!(script.contains("printf '\\n'"));
    }
}
