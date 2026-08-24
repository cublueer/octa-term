//! bash 钩子：单行兜底路线。
//!
//! bash 没有可靠的 Enter 劫持（`bind -x` 三期再做），这里只接管
//! `command_not_found_handle`：单行未知命令在报错前交给 octa-term 分类，
//! 数学表达式就地求值，其余返回 127 让 bash 正常报错。

use std::path::PathBuf;

use anyhow::Result;

use crate::paths::Paths;

pub fn hook() -> String {
    hook_with_bin(&super::hook_bin())
}

/// 用指定二进制路径生成钩子（回归测试注入不存在的路径用）。
pub fn hook_with_bin(bin: &str) -> String {
    format!(
        r#"command_not_found_handle() {{
    [[ $- == *i* ]] || return 127

    local text="$*"
    [[ -n "$text" ]] || return 127
    [[ "$text" != *$'\n'* && "$text" != *$'\r'* ]] || return 127

    # 递归守卫：octa-term 自身找不到时绝不能再次调用它
    if ! command -v {bin} >/dev/null 2>&1; then
        printf 'bash: %s: command not found\n' "$1" >&2
        return 127
    fi

    # 数学求值成功（exit 0）就不再报错；不是数学时补回标准提示
    {bin} --shell-intercept --shell bash -- "$@" 2>/dev/null \
        || printf 'bash: %s: command not found\n' "$1" >&2
    return 127
}}
"#
    )
}

pub fn install(paths: &Paths) -> Result<()> {
    if let Some(parent) = paths.bash_hook_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&paths.bash_hook_file, hook())?;
    let rc_path = home_file(".bashrc");
    super::upsert_source_block(
        &rc_path,
        super::BASH_BEGIN_MARKER,
        super::BASH_END_MARKER,
        &paths.bash_hook_file,
    )?;
    println!("已安装 bash hook：{}", paths.bash_hook_file.display());
    println!("已更新：{}", rc_path.display());
    super::warn_if_binary_missing_from_path();
    super::print_reload_hint("bash", &paths.bash_hook_file);
    Ok(())
}

pub fn uninstall(paths: &Paths) -> Result<bool> {
    let removed_file = match std::fs::remove_file(&paths.bash_hook_file) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(err.into()),
    };
    let rc_path = home_file(".bashrc");
    let removed_block = super::remove_source_block(
        &rc_path,
        super::BASH_BEGIN_MARKER,
        super::BASH_END_MARKER,
    )?;
    let removed = removed_file || removed_block;
    if removed {
        println!("已移除 bash hook");
    }
    Ok(removed)
}

fn home_file(name: &str) -> PathBuf {
    directories_home().join(name)
}

fn directories_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_defines_command_not_found_handler() {
        let script = hook();
        assert!(script.contains("command_not_found_handle"));
        assert!(script.contains("--shell bash"));
        assert!(script.contains("return 127"));
        assert!(script.contains("$- == *i*"));
        // 缺二进制守卫：octa-term 不在 PATH 时不调用，防递归
        // 缺二进制守卫：二进制不可用时绝不调用，防递归
        assert!(script.contains("command -v "));
    }
}
