//! 冷调用求值：每个表达式开一个独立 `octave --no-gui --quiet
//! --no-init-file` 子进程，stdin 送入表达式，超时 SIGKILL 整个进程。
//! 无状态、崩溃域小；daemon 常驻模式（阶段 5）与此并行存在。

use std::io::IsTerminal;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Error,
    Timeout,
    Blocked,
}

pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub status: Status,
    pub duration_ms: u128,
}

/// 求值并清洗输出。安全过滤失败时返回 `Status::Blocked` 的 Outcome
/// （不产生子进程）。
pub async fn evaluate(expr: &str, limit: Duration) -> anyhow::Result<Outcome> {
    let start = Instant::now();
    let mut child = Command::new("octave")
        .args(["--no-gui", "--quiet", "--no-init-file"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // 超时取消 wait future 时由 tokio 后台收尸（SIGKILL + reap）
        .kill_on_drop(true)
        .spawn()?;

    let mut stdin = child.stdin.take().expect("stdin piped");
    // 必须 spawn：惰性 future 若与 wait 同链 poll 会互相饿死（stdin 没人写，
    // octave 永远等输入 → 恒超时）。
    let input = expr.to_owned();
    let write_task = tokio::spawn(async move {
        stdin.write_all(input.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        // drop stdin → EOF → octave 退出
        Ok::<(), std::io::Error>(())
    });

    match timeout(limit, child.wait_with_output()).await {
        Err(_elapsed) => {
            // kill_on_drop(true)：wait future 此刻被丢弃，tokio 后台发 SIGKILL 并收尸
            let _ = write_task.await;
            Ok(Outcome {
                stdout: String::new(),
                stderr: String::new(),
                status: Status::Timeout,
                duration_ms: start.elapsed().as_millis(),
            })
        }
        Ok(Err(err)) => {
            let _ = write_task.await;
            Err(err.into())
        }
        Ok(Ok(output)) => {
            write_task.await??;
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = clean_stderr(&String::from_utf8_lossy(&output.stderr));
            let status = if !output.status.success() && stdout.trim().is_empty() {
                Status::Error
            } else {
                Status::Ok
            };
            Ok(Outcome {
                stdout,
                stderr,
                status,
                duration_ms: start.elapsed().as_millis(),
            })
        }
    }
}

/// 过滤 Octave 退出时的固定噪音行（11.3 的 `const execution_exception`
/// 是无害的退出路径告警）与 GUI/字体探测告警。
fn clean_stderr(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty()
                && !line.contains("execution_exception")
                && !line.contains("No working Wayland or X11 display")
                && !line.contains("disabling GUI features")
                && !line.contains("fontconfig")
                && !line.contains("Fontconfig")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 把求值结果打印到终端：stdout 原样透传（保留 `ans = …` 原生格式），
/// 静默求值（末尾 `;` 无输出）印淡色确认，错误/超时走淡红色提示。
pub fn print_outcome(outcome: &Outcome) {
    match outcome.status {
        Status::Timeout => {
            println!(
                "{}",
                dim_red(&format!(
                    "⚠ 计算超时（{}s），已中止",
                    outcome.duration_ms as f64 / 1000.0
                ))
            );
        }
        Status::Blocked => {
            println!("{}", dim_red("⚠ 表达式被安全策略拦截"));
        }
        Status::Error => {
            if !outcome.stdout.trim().is_empty() {
                print!("{}", outcome.stdout);
            }
            if !outcome.stderr.trim().is_empty() {
                eprintln!("{}", dim_red(outcome.stderr.trim_end()));
            }
        }
        Status::Ok => {
            if outcome.stdout.trim().is_empty() {
                println!("{}", dim("✓ 已求值（静默）"));
            } else {
                print!("{}", outcome.stdout);
            }
        }
    }
}

fn dim(text: &str) -> String {
    if std::io::stdout().is_terminal() {
        format!("\x1b[2m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn dim_red(text: &str) -> String {
    if std::io::stdout().is_terminal() {
        format!("\x1b[2;31m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}
