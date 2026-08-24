//! daemon 本体：flock 单实例 + Unix socket 服务 + 会话线程。
//!
//! 结构：
//! - 主循环（tokio）：接受连接、解帧、把求值任务经 mpsc 交给会话线程，
//!   用 oneshot 等结果再回帧。请求天然串行。
//! - 会话线程（std::thread）：独占 OctaveSession（pty），阻塞式求值。
//! - 控制帧：ping / shutdown；shutdown 时清理 socket、pid 并杀掉 octave。

pub mod client;
pub mod control;
pub mod ipc;
pub mod session;

use std::io::Write as _;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};

use crate::classify;
use crate::paths::Paths;

struct Job {
    expr: String,
    timeout: Duration,
    reply: oneshot::Sender<ipc::Response>,
}

/// daemon 主入口（`octa-term __daemon`）。
pub async fn run(paths: &Paths) -> Result<()> {
    paths.ensure_state_dir()?;
    let _lock = acquire_lock(&paths.lock_path())?;
    log_line(paths, "daemon 启动");
    std::fs::write(paths.pid_path(), std::process::id().to_string())?;

    // 清掉可能残留的 socket（上一个实例非正常退出）
    let socket_path = paths.socket_path();
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("bind {}", socket_path.display()))?;

    // 会话线程：独占 pty 会话
    let (job_tx, job_rx) = mpsc::channel::<Job>(4);
    let session_paths = paths.clone();
    std::thread::spawn(move || session_loop(job_rx, session_paths));

    let mut shutdown = false;
    while !shutdown {
        let (stream, _addr) = listener.accept().await?;
        match handle_connection(stream, &job_tx).await {
            Ok(Some(ipc::Control::Shutdown)) => shutdown = true,
            Ok(_) => {}
            Err(err) => log_line(paths, &format!("连接处理失败: {err:#}")),
        }
    }

    log_line(paths, "daemon 收到 shutdown，清理退出");
    drop(job_tx); // 会话线程随之结束并杀掉 octave
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(paths.pid_path());
    Ok(())
}

async fn handle_connection(
    stream: UnixStream,
    job_tx: &mpsc::Sender<Job>,
) -> Result<Option<ipc::Control>> {
    let (mut reader, mut writer) = stream.into_split();
    let frame = tokio::time::timeout(Duration::from_secs(10), ipc::read_frame(&mut reader))
        .await
        .context("读请求帧超时")??;
    let control: ipc::Control = serde_json::from_slice(&frame)?;

    match &control {
        ipc::Control::Ping => {
            let payload = serde_json::to_vec(&serde_json::json!({"ok": true}))?;
            ipc::write_frame(&mut writer, &payload).await?;
        }
        ipc::Control::Shutdown => {
            let payload = serde_json::to_vec(&serde_json::json!({"ok": true}))?;
            ipc::write_frame(&mut writer, &payload).await?;
        }
        ipc::Control::Eval {
            expr,
            timeout_secs,
        } => {
            // daemon 侧同样执行安全过滤（防御纵深：不信任客户端输入）
            let response = match classify::safety::check(expr) {
                Err(reason) => ipc::Response {
                    output: format!("已拦截：{reason}"),
                    status: "blocked".to_string(),
                    duration_ms: 0,
                },
                Ok(()) => {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    let timeout = Duration::from_secs_f64(timeout_secs.max(0.1));
                    job_tx
                        .send(Job {
                            expr: expr.clone(),
                            timeout,
                            reply: reply_tx,
                        })
                        .await
                        .context("会话线程已退出")?;
                    // 上限 = 求值超时 + SIGINT 宽限 + 余量
                    let overall = timeout + Duration::from_secs(8);
                    tokio::time::timeout(overall, reply_rx)
                        .await
                        .context("等会话结果超时")?
                        .context("会话线程断连")?
                }
            };
            let payload = serde_json::to_vec(&response)?;
            ipc::write_frame(&mut writer, &payload).await?;
        }
    }
    writer.shutdown().await.ok();
    Ok(Some(control))
}

fn session_loop(mut job_rx: mpsc::Receiver<Job>, paths: Paths) {
    // 懒启动：octave 拉起失败不致命，每次求值前重试一次
    let mut session: Option<session::OctaveSession> = session::OctaveSession::spawn()
        .map_err(|err| log_line(&paths, &format!("octave 会话启动失败: {err:#}")))
        .ok();

    while let Some(job) = job_rx.blocking_recv() {
        let result = match session.as_mut() {
            Some(handle) => handle.eval(&job.expr, job.timeout),
            None => session::EvalResult {
                output: "octave 会话不可用（启动失败）".to_string(),
                status: session::EvalStatus::SessionReset,
                duration_ms: 0,
            },
        };
        let response = ipc::Response {
            output: result.output,
            status: result.status.as_str().to_string(),
            duration_ms: result.duration_ms,
        };
        if job.reply.send(response).is_err() {
            log_line(&paths, "客户端已断开，结果丢弃");
        }
    }

    if let Some(mut handle) = session.take() {
        handle.kill();
    }
    log_line(&paths, "会话线程退出");
}

/// flock 单实例：拿不到锁说明已有 daemon 在跑。
fn acquire_lock(path: &std::path::Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    // SAFETY: flock 参数由 Rust 侧构造
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        bail!("已有 octa-term daemon 在运行（{}）", path.display());
    }
    Ok(file)
}

pub fn log_line(paths: &Paths, message: &str) {
    let line = format!(
        "{} {message}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.daemon_log())
    {
        let _ = file.write_all(line.as_bytes());
    }
}
