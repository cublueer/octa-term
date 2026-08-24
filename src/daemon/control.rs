//! daemon 控制命令：start / stop / restart / status / logs / enable / disable。
//!
//! `enable` 安装 systemd user unit（登录自启 + 立即启动），`disable` 反向。

use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::client;
use crate::args::{DaemonArgs, DaemonCommand, DaemonLogsArgs};
use crate::paths::Paths;

pub async fn run(paths: &Paths, args: &DaemonArgs) -> Result<()> {
    match args.command.as_ref().unwrap_or(&DaemonCommand::Status) {
        DaemonCommand::Start => start(paths).await,
        DaemonCommand::Stop => stop(paths).await,
        DaemonCommand::Restart => {
            let _ = stop(paths).await;
            start(paths).await
        }
        DaemonCommand::Status => status(paths).await,
        DaemonCommand::Logs(logs) => logs_cmd(paths, logs),
        DaemonCommand::Enable => enable(paths),
        DaemonCommand::Disable => disable(paths),
    }
}

async fn start(paths: &Paths) -> Result<()> {
    if client::ping(paths).await {
        println!("daemon 已在运行");
        return Ok(());
    }
    let exe = std::env::current_exe().context("找不到当前可执行文件")?;
    std::process::Command::new(exe)
        .arg("__daemon")
        .env("OCTA_STATE_DIR", &paths.state_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("拉起 daemon 失败")?;
    for _ in 0..50 {
        if client::ping(paths).await {
            let pid = std::fs::read_to_string(paths.pid_path())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "?".to_string());
            println!("daemon 已启动（pid {pid}）");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!(
        "daemon 启动超时，查看日志：octa-term daemon logs（{}）",
        paths.daemon_log().display()
    )
}

async fn stop(paths: &Paths) -> Result<()> {
    if !client::ping(paths).await {
        println!("daemon 未在运行");
        return Ok(());
    }
    if client::shutdown(paths).await {
        // 等 socket 消失
        for _ in 0..30 {
            if !client::ping(paths).await {
                println!("daemon 已停止");
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    bail!("daemon 未能优雅退出；可手动 kill（pid 文件: {}）", paths.pid_path().display())
}

async fn status(paths: &Paths) -> Result<()> {
    if client::ping(paths).await {
        let pid = std::fs::read_to_string(paths.pid_path())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "?".to_string());
        println!("daemon 运行中（pid {pid}，socket {}）", paths.socket_path().display());
    } else {
        println!("daemon 未在运行（socket {}）", paths.socket_path().display());
    }
    Ok(())
}

fn logs_cmd(paths: &Paths, args: &DaemonLogsArgs) -> Result<()> {
    let content = std::fs::read_to_string(paths.daemon_log()).unwrap_or_default();
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(args.lines);
    for line in &lines[start..] {
        println!("{line}");
    }
    if content.is_empty() {
        println!("（暂无日志）");
    }
    Ok(())
}

const UNIT_NAME: &str = "octa-term.service";

fn enable(paths: &Paths) -> Result<()> {
    let exe = std::env::current_exe().context("找不到当前可执行文件")?;
    let unit = unit_content(exe.to_str().unwrap_or_default());
    let unit_dir = paths.config_home.join("systemd/user");
    std::fs::create_dir_all(&unit_dir)?;
    let unit_path = unit_dir.join(UNIT_NAME);
    std::fs::write(&unit_path, unit)?;
    run_systemctl(&["--user", "daemon-reload"])?;
    run_systemctl(&["--user", "enable", "--now", UNIT_NAME])?;
    println!(
        "已安装并启动 systemd user unit：{}\n登录自启已生效；管理命令：octa-term daemon stop/restart/status/logs",
        unit_path.display()
    );
    Ok(())
}

/// systemd user unit 模板（纯函数，便于测试）。
fn unit_content(exe: &str) -> String {
    format!(
        "[Unit]\n\
         Description=octa-term daemon (seamless terminal math)\n\
         After=default.target\n\n\
         [Service]\n\
         Type=simple\n\
         ExecStart={}\n\
         Restart=on-failure\n\
         RestartSec=2\n\n\
         [Install]\n\
         WantedBy=default.target\n",
        shell_escape(exe)
    )
}

fn disable(paths: &Paths) -> Result<()> {
    let _ = run_systemctl(&["--user", "disable", "--now", UNIT_NAME]);
    let unit_path = paths.config_home.join("systemd/user").join(UNIT_NAME);
    match std::fs::remove_file(&unit_path) {
        Ok(()) => println!("已移除 {}", unit_path.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!("unit 文件不存在（可能已移除）")
        }
        Err(err) => return Err(err.into()),
    }
    let _ = run_systemctl(&["--user", "daemon-reload"]);
    Ok(())
}

fn run_systemctl(args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("systemctl")
        .args(args)
        .status()
        .context("执行 systemctl 失败")?;
    if !status.success() {
        bail!("systemctl {} 退出码非零", args.join(" "));
    }
    Ok(())
}

/// ExecStart 一行内用引号包裹路径（systemd 按空格分词）。
fn shell_escape(path: &str) -> String {
    format!("\"{}\"", path.replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_content_quotes_path_and_declares_autostart() {
        let unit = unit_content("/home/me/bin/octa-term");
        assert!(unit.contains("ExecStart=\"/home/me/bin/octa-term\""));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.contains("Restart=on-failure"));
    }

    #[test]
    fn unit_content_escapes_embedded_quotes() {
        let unit = unit_content("/we\"ird/octa-term");
        assert!(unit.contains("ExecStart=\"/we\\\"ird/octa-term\""));
    }
}
