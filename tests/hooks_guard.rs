//! 递归守卫回归测试：用户真机发现的「octa-term 不在 PATH → handler 再调
//! octa-term → 无限递归」。这里把不存在的二进制路径注入钩子，直接调用
//! command_not_found handler，断言**恰好一次**报错且返回 127。

use std::process::Command;

use octa_term::hooks;

/// 钩子里注入的、必然不存在的二进制路径。
const MISSING_BIN: &str = "/nonexistent/octa-term";

fn write_hook(name: &str, content: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("octa-guard-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn shell_available(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn fish_handler_does_not_recurse_when_binary_missing() {
    if !shell_available("fish") {
        eprintln!("跳过：无 fish");
        return;
    }
    let hook = write_hook("guard.fish", &hooks::fish::hook_with_bin(MISSING_BIN));
    // -i 强制交互态（handler 首行要求），-c 直接调用 handler
    let output = Command::new("fish")
        .args([
            "-i",
            "-c",
            &format!(
                "source '{}'; fish_command_not_found {MISSING_BIN} --shell-intercept --shell fish -- foo; echo STATUS=$status",
                hook.display()
            ),
        ])
        .output()
        .expect("run fish");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        text.matches("Unknown command").count(),
        1,
        "应恰好报错一次，实际输出:\n{text}"
    );
    assert!(text.contains("STATUS=127"), "输出:\n{text}");
}

#[test]
fn bash_handler_does_not_recurse_when_binary_missing() {
    if !shell_available("bash") {
        eprintln!("跳过：无 bash");
        return;
    }
    let hook = write_hook("guard.sh", &hooks::bash::hook_with_bin(MISSING_BIN));
    let output = Command::new("bash")
        .args([
            "-ic",
            &format!(
                "source '{}'; command_not_found_handle {MISSING_BIN} --shell-intercept foo; echo STATUS=$?",
                hook.display()
            ),
        ])
        .output()
        .expect("run bash");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        text.matches("command not found").count(),
        1,
        "应恰好报错一次，实际输出:\n{text}"
    );
    assert!(text.contains("STATUS=127"), "输出:\n{text}");
}

#[test]
fn zsh_handler_does_not_recurse_when_binary_missing() {
    if !shell_available("zsh") {
        eprintln!("跳过：无 zsh");
        return;
    }
    let hook = write_hook("guard.zsh", &hooks::zsh::hook_with_bin(MISSING_BIN));
    let output = Command::new("zsh")
        .args([
            "-ic",
            &format!(
                "source '{}'; command_not_found_handler {MISSING_BIN} --shell-intercept foo; echo STATUS=$?",
                hook.display()
            ),
        ])
        .output()
        .expect("run zsh");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        text.matches("command not found").count(),
        1,
        "应恰好报错一次，实际输出:\n{text}"
    );
    assert!(text.contains("STATUS=127"), "输出:\n{text}");
}
