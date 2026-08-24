//! 常驻 Octave 会话：pty 承载的 octave REPL，`PS1` 标记同步求值边界。
//!
//! 为什么必须 pty：octave 只在「看起来是交互终端」时才进入 REPL 并打印
//! 提示符，管道模式一次读入全部输入后直接退出（实测已确认）。本模块用
//! libc openpty 造一个伪终端，关闭回显，用自定义提示符 `__OCTA_READY__`
//! 作为「上一条语句已完成」的同步标记。
//!
//! 超时策略（保变量优先）：到期先向 pty 写 `\x03`（SIGINT，octave 中断
//! 当前语句、工作区保留）；宽限期内没回到提示符才强杀重生。

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// 初始握手与单条语句的读取上限兜底
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// SIGINT 后等待回到提示符的宽限期
const INTERRUPT_GRACE: Duration = Duration::from_secs(3);

/// 同步标记必须每次会话随机：固定串会被用户 `disp('__OCTA_READY__')`
/// 这类输出伪造，导致会话同步永久错乱（实测）。
fn make_prompt_marker() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("__OCTA_READY_{}_{nanos}__", std::process::id())
}

pub enum EvalStatus {
    Ok,
    Error,
    Timeout,
    SessionReset,
}

impl EvalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            EvalStatus::Ok => "ok",
            EvalStatus::Error => "error",
            EvalStatus::Timeout => "timeout",
            EvalStatus::SessionReset => "session_reset",
        }
    }
}

pub struct EvalResult {
    pub output: String,
    pub status: EvalStatus,
    pub duration_ms: u64,
}

pub struct OctaveSession {
    child: Child,
    master: std::fs::File,
    /// 本次会话的同步标记（随机，防止用户输出伪造）
    prompt: String,
    /// 上次求值后已损坏（进程死亡且重生失败）标记
    broken: bool,
}

impl OctaveSession {
    pub fn spawn() -> Result<OctaveSession> {
        let mut master_fd: libc::c_int = -1;
        let mut slave_fd: libc::c_int = -1;
        let winsize = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty 输出两个新 fd；失败时 errno 已设置
        let rc = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                &winsize,
            )
        };
        if rc != 0 {
            anyhow::bail!(
                "openpty 失败: {}",
                std::io::Error::last_os_error()
            );
        }
        let slave = unsafe { OwnedFd::from_raw_fd(slave_fd) };
        // SAFETY: pre_exec 闭包只做 setsid/TIOCSCTTY，无内存安全问题
        // 注意：octave 11.3 在 tty 下收到 --eval 会直接退出（实测），所以
        // PS1 必须在握手阶段通过 stdin 发送，不能用 --eval。
        let child = unsafe {
            Command::new("octave")
                .args([
                    "--no-gui",
                    "--quiet",
                    "--no-init-file",
                    "--no-line-editing",
                ])
                .stdin(Stdio::from(dup_fd(&slave)?))
                .stdout(Stdio::from(dup_fd(&slave)?))
                .stderr(Stdio::from(dup_fd(&slave)?))
                .pre_exec(|| {
                    // 新会话 + 抢占控制终端，octave 才按交互 REPL 运行
                    libc::setsid();
                    libc::ioctl(0, libc::TIOCSCTTY, 0);
                    Ok(())
                })
                .spawn()
                .context("spawn octave")?
        };
        drop(slave);

        let master = unsafe { std::fs::File::from_raw_fd(master_fd) };
        disable_echo(&master);
        let prompt = make_prompt_marker();
        let mut session = OctaveSession {
            child,
            master,
            prompt,
            broken: false,
        };
        // 握手：等默认提示符 `octave:N>` → 设置自定义 PS1 → 等新标记
        session
            .read_until_default_prompt(HANDSHAKE_TIMEOUT)
            .context("等待 octave 初始提示符超时")?;
        session
            .master
            .write_all(format!("PS1('{}');\n", session.prompt).as_bytes())?;
        let prompt_marker = session.prompt.clone();
        session
            .read_until(prompt_marker.as_bytes(), HANDSHAKE_TIMEOUT)
            .context("等待 octave 自定义提示符超时")?;
        Ok(session)
    }

    /// 求值一条表达式。任何 IO 级故障都会尝试重生会话并返回 SessionReset。
    pub fn eval(&mut self, expr: &str, timeout: Duration) -> EvalResult {
        let start = Instant::now();
        match self.try_eval(expr, timeout) {
            Ok((output, status)) => EvalResult {
                output,
                status,
                duration_ms: start.elapsed().as_millis() as u64,
            },
            Err(err) => {
                self.broken = true;
                EvalResult {
                    output: format!("会话故障，已重启：{err}"),
                    status: EvalStatus::SessionReset,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }
        }
    }

    fn try_eval(&mut self, expr: &str, timeout: Duration) -> Result<(String, EvalStatus)> {
        // 进程已死？先收尸再重生
        if let Some(_status) = self.child.try_wait()? {
            self.respawn()?;
            return Ok((
                "会话曾中断，已重生（此前变量已丢失）".to_string(),
                EvalStatus::SessionReset,
            ));
        }

        self.master.write_all(expr.as_bytes())?;
        self.master.write_all(b"\n")?;

        let prompt = self.prompt.clone();
        match self.read_until(prompt.as_bytes(), timeout) {
            Ok(buf) => {
                let output = clean_continuation(&extract_before_marker(&buf, prompt.as_bytes()));
                let status = if has_error_line(&output) {
                    EvalStatus::Error
                } else {
                    EvalStatus::Ok
                };
                Ok((output, status))
            }
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {
                // 超时：SIGINT 中断当前语句（保变量），宽限期内回到提示符
                self.master.write_all(b"\x03")?;
                match self.read_until(prompt.as_bytes(), INTERRUPT_GRACE) {
                    Ok(_) => Ok((
                        format!("⚠ 计算超时（{}s），已中断", timeout.as_secs_f64()),
                        EvalStatus::Timeout,
                    )),
                    Err(_) => {
                        self.respawn()?;
                        Ok((
                            "⚠ 计算超时且无法中断，会话已重启（变量已丢失）".to_string(),
                            EvalStatus::SessionReset,
                        ))
                    }
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// 读数据直到出现标记，或超时（返回 TimedOut）。返回的内容**包含**标记。
    fn read_until(&mut self, needle: &[u8], limit: Duration) -> std::io::Result<Vec<u8>> {
        self.read_until_pred(limit, |buf| find_subslice(buf, needle).is_some())
    }

    /// 等默认提示符 `octave:N>`（N 随命令数递增，不能当常量）。
    fn read_until_default_prompt(&mut self, limit: Duration) -> std::io::Result<Vec<u8>> {
        self.read_until_pred(limit, |buf| {
            // 找最后一次出现的 "octave:"，其后到 ">" 之间全是数字
            let Some(pos) = buf.windows(7).rposition(|w| w == b"octave:") else {
                return false;
            };
            let rest = &buf[pos + 7..];
            let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
            digits > 0 && rest.get(digits) == Some(&b'>')
        })
    }

    fn read_until_pred(
        &mut self,
        limit: Duration,
        mut done: impl FnMut(&[u8]) -> bool,
    ) -> std::io::Result<Vec<u8>> {
        let mut buf: Vec<u8> = Vec::new();
        let deadline = Instant::now() + limit;
        loop {
            if done(&buf) {
                return Ok(buf);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "read_until timeout",
                ));
            }
            if !poll_readable(&self.master, remaining)? {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "read_until timeout",
                ));
            }
            let mut chunk = [0u8; 4096];
            let n = self.master.read(&mut chunk)?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "octave pty closed",
                ));
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// 强杀并重生 octave（变量丢失，历史在数据库里不丢）。
    fn respawn(&mut self) -> Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        *self = OctaveSession::spawn()?;
        Ok(())
    }

    pub fn kill(&mut self) {
        if !self.broken {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.broken = true;
        }
    }
}

impl Drop for OctaveSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn dup_fd(fd: &OwnedFd) -> Result<OwnedFd> {
    // SAFETY: dup 现有 fd；成功后返回新 fd
    let new_fd = unsafe { libc::dup(fd.as_raw_fd()) };
    if new_fd < 0 {
        anyhow::bail!("dup 失败: {}", std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(new_fd) })
}

fn disable_echo(master: &std::fs::File) {
    // SAFETY: termios 结构体由 tcgetattr 填充后按位修改
    unsafe {
        let mut term: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(master.as_raw_fd(), &mut term) == 0 {
            term.c_lflag &= !libc::ECHO;
            libc::tcsetattr(master.as_raw_fd(), libc::TCSANOW, &term);
        }
    }
}

fn poll_readable(fd: &std::fs::File, timeout: Duration) -> std::io::Result<bool> {
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: poll 只读 pfd
    let rc = unsafe { libc::poll(&mut pfd, 1, millis) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if rc == 0 {
        return Ok(false);
    }
    if pfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
        return Ok(true);
    }
    Ok(false)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 标记之前的全部内容即求值输出（回显已关）。
fn extract_before_marker(buf: &[u8], marker: &[u8]) -> String {
    let end = find_subslice(buf, marker).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// 多行语句会触发 Octave 续行提示符（PS2 = `> `），它们排在结果前面，
/// 全部剥掉（回显已关，不会有输入行混入）。
fn clean_continuation(output: &str) -> String {
    let mut current = output;
    while let Some(stripped) = current.strip_prefix("> ") {
        current = stripped;
    }
    current.to_string()
}

fn has_error_line(output: &str) -> bool {
    output.lines().any(|line| line.trim_start().starts_with("error:"))
}
