//! shell 钩子：安装 / 卸载与标记块管理。
//!
//! - fish：把脚本直接写进 `conf.d`（fish 自动加载），卸载删文件
//! - bash/zsh：钩子脚本写到配置目录，并在 `~/.bashrc`/`~/.zshrc` 里用
//!   标记行包住一个 `source` 块——重装只替换块内内容，卸载精确摘除，
//!   全程原子写（临时文件 + rename），写坏 rc 的概率为零

pub mod bash;
pub mod fish;
pub mod zsh;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{bail, Context, Result};

pub const BASH_BEGIN_MARKER: &str = "# >>> octa-term bash hook >>>";
pub const BASH_END_MARKER: &str = "# <<< octa-term bash hook <<<";
pub const ZSH_BEGIN_MARKER: &str = "# >>> octa-term zsh hook >>>";
pub const ZSH_END_MARKER: &str = "# <<< octa-term zsh hook <<<";

/// 原子写用户 shell 启动文件：写回瞬间崩溃不能把 rc 留成截断的半个文件。
pub(super) fn write_rc_atomic(rc_path: &Path, content: &str) -> Result<()> {
    let parent = rc_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mode = fs::metadata(rc_path)
        .ok()
        .map(|metadata| metadata.permissions().mode() & 0o7777)
        .unwrap_or(0o644);
    let temporary = parent.join(format!(
        ".octa-hook-{}-{}",
        std::process::id(),
        rand_token()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(mode)
        .open(&temporary)
        .with_context(|| format!("creating temp file next to {}", rc_path.display()))?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, rc_path)
        .with_context(|| format!("updating shell startup file {}", rc_path.display()))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn rand_token() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// 幂等地在 rc 里维护 source 块：已有标记块 → 原位替换内容；
/// 没有 → 追加到文件末尾。
pub(super) fn upsert_source_block(
    rc_path: &Path,
    begin: &str,
    end: &str,
    hook_file: &Path,
) -> Result<()> {
    let existing = read_optional_text(rc_path)?;
    let block = source_block(begin, end, hook_file);
    if let Some(updated) = replace_marked_block(&existing, begin, end, &block)? {
        if updated != existing {
            write_rc_atomic(rc_path, &updated)?;
        }
        return Ok(());
    }
    if let Some(parent) = rc_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(rc_path)?;
    if !existing.ends_with('\n') && !existing.is_empty() {
        writeln!(file)?;
    }
    file.write_all(block.as_bytes())?;
    Ok(())
}

/// 精确摘除标记块；rc 不存在或没有标记时不动文件。
pub(super) fn remove_source_block(rc_path: &Path, begin: &str, end: &str) -> Result<bool> {
    let Ok(existing) = fs::read_to_string(rc_path) else {
        return Ok(false);
    };
    let Some(begin_index) = existing.find(begin) else {
        return Ok(false);
    };
    let Some(end_relative) = existing[begin_index..].find(end) else {
        return Ok(false);
    };
    let mut end_index = begin_index + end_relative + end.len();
    if existing.as_bytes().get(end_index) == Some(&b'\r') {
        end_index += 1;
    }
    if existing.as_bytes().get(end_index) == Some(&b'\n') {
        end_index += 1;
    }
    let mut updated = String::new();
    updated.push_str(&existing[..begin_index]);
    updated.push_str(&existing[end_index..]);
    write_rc_atomic(rc_path, &updated)?;
    Ok(true)
}

fn read_optional_text(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.into()),
    }
}

fn source_block(begin: &str, end: &str, hook_file: &Path) -> String {
    let hook = shell_quote(hook_file);
    format!("{begin}\n[ -r {hook} ] && source {hook}\n{end}\n")
}

fn replace_marked_block(
    existing: &str,
    begin: &str,
    end: &str,
    replacement: &str,
) -> Result<Option<String>> {
    let Some(begin_index) = existing.find(begin) else {
        if existing.contains(end) {
            bail!("shell 启动文件里有 octa-term 结束标记却没有开始标记");
        }
        return Ok(None);
    };
    let Some(end_relative) = existing[begin_index..].find(end) else {
        bail!("shell 启动文件里有不完整的 octa-term 钩子块");
    };
    let mut end_index = begin_index + end_relative + end.len();
    if existing.as_bytes().get(end_index) == Some(&b'\r') {
        end_index += 1;
    }
    if existing.as_bytes().get(end_index) == Some(&b'\n') {
        end_index += 1;
    }
    let mut updated = String::with_capacity(
        existing.len().saturating_sub(end_index - begin_index) + replacement.len(),
    );
    updated.push_str(&existing[..begin_index]);
    updated.push_str(replacement);
    updated.push_str(&existing[end_index..]);
    Ok(Some(updated))
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// 卸载全部三个钩子；一个都没装时给出提示。
pub fn remove_all(paths: &crate::paths::Paths) -> Result<()> {
    let removed = fish::uninstall(paths)?;
    let removed = bash::uninstall(paths)? || removed;
    let removed = zsh::uninstall(paths)? || removed;
    if !removed {
        println!("未找到已安装的 octa-term shell hook");
    }
    Ok(())
}

/// 卸载后打印重载提示：当前父进程就是对应 shell 时给 source 命令，
/// 否则提示新开会话。
pub(super) fn print_reload_hint(shell: &str, hook_file: &Path) {
    let source = format!("source '{}'", hook_file.display());
    if current_parent_shell().as_deref() == Some(shell) {
        println!("在当前终端运行此命令可立即加载：{source}");
    } else {
        println!("新开对应 shell 会话后 hook 将生效（或手动 {source}）");
    }
}

/// `octa-term` 是否能在 PATH 中解析。钩子在 shell 里按名字调用它；
/// 用户以完整路径运行 `octa-term fish-init` 时二进制未必已安装，
/// 此时钩子运行时会被守卫禁用——安装完必须提示。
pub(super) fn binary_in_path() -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join("octa-term");
        fs::metadata(&candidate)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

pub(super) fn warn_if_binary_missing_from_path() {
    if !binary_in_path() {
        println!(
            "ℹ octa-term 不在 PATH 中：hook 已烙入当前可执行文件的绝对路径，\
             立即可用。建议正式安装（如 install -m755 target/release/octa-term \
             ~/.local/bin/）后重新运行本命令，之后 hook 会改用 PATH 解析（升级更省心）。"
        );
    }
}

/// 钩子内调用的二进制路径，优先级：
/// 1. `OCTA_BIN` 显式覆盖（测试用）
/// 2. PATH 可解析 → 裸名 `octa-term`（升级/移动后仍按 PATH 找到）
/// 3. 都不行 → 当前可执行文件的绝对路径（未安装也能立即工作；文件消失时
///    由钩子内的存在性守卫退化为原生行为，不会递归）
pub(super) fn hook_bin() -> String {
    if let Ok(bin) = std::env::var("OCTA_BIN") {
        if !bin.is_empty() {
            return bin;
        }
    }
    if binary_in_path() {
        return "octa-term".to_string();
    }
    std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "octa-term".to_string())
}

pub(super) fn current_parent_shell() -> Option<String> {
    let mut pid = std::process::id();
    for _ in 0..8 {
        let parent = parent_pid(pid)?;
        let name = process_name(parent)?;
        if matches!(name.as_str(), "fish" | "bash" | "zsh") {
            return Some(name);
        }
        pid = parent;
    }
    None
}

fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.rsplit_once(") ")?.1;
    after_name.split_whitespace().nth(1)?.parse().ok()
}

fn process_name(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_rc() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rc");
        (dir, path)
    }

    #[test]
    fn upsert_appends_when_missing() {
        let (_dir, rc) = temp_rc();
        let hook = PathBuf::from("/tmp/my hook.sh");
        upsert_source_block(&rc, BASH_BEGIN_MARKER, BASH_END_MARKER, &hook).unwrap();
        let content = fs::read_to_string(&rc).unwrap();
        assert!(content.contains("# >>> octa-term bash hook >>>"));
        assert!(content.contains("'/tmp/my hook.sh'"));
        assert!(content.contains("# <<< octa-term bash hook <<<"));
    }

    #[test]
    fn upsert_refreshes_existing_block_without_duplicates() {
        let (_dir, rc) = temp_rc();
        let hook = PathBuf::from("/old/hook.sh");
        upsert_source_block(&rc, BASH_BEGIN_MARKER, BASH_END_MARKER, &hook).unwrap();
        fs::write(
            &rc,
            format!(
                "before\n{BASH_BEGIN_MARKER}\nsource '/old/hook.sh'\n{BASH_END_MARKER}\nafter\n"
            ),
        )
        .unwrap();
        let new_hook = PathBuf::from("/new/hook.sh");
        upsert_source_block(&rc, BASH_BEGIN_MARKER, BASH_END_MARKER, &new_hook).unwrap();
        let content = fs::read_to_string(&rc).unwrap();
        assert!(content.contains("/new/hook.sh"));
        assert!(!content.contains("/old/hook.sh"));
        assert_eq!(content.matches(BASH_BEGIN_MARKER).count(), 1);
        assert!(content.starts_with("before\n"));
        assert!(content.ends_with("after\n"));
    }

    #[test]
    fn remove_block_restores_original() {
        let (_dir, rc) = temp_rc();
        fs::write(
            &rc,
            format!("before\n{BASH_BEGIN_MARKER}\nsource hook\n{BASH_END_MARKER}\nafter\n"),
        )
        .unwrap();
        assert!(remove_source_block(&rc, BASH_BEGIN_MARKER, BASH_END_MARKER).unwrap());
        assert_eq!(fs::read_to_string(&rc).unwrap(), "before\nafter\n");
        assert!(!remove_source_block(&rc, BASH_BEGIN_MARKER, BASH_END_MARKER).unwrap());
    }

    #[test]
    fn end_marker_without_begin_is_rejected() {
        let (_dir, rc) = temp_rc();
        fs::write(&rc, format!("{BASH_END_MARKER}\n")).unwrap();
        let hook = PathBuf::from("/tmp/hook.sh");
        assert!(upsert_source_block(&rc, BASH_BEGIN_MARKER, BASH_END_MARKER, &hook).is_err());
    }
}
