//! 命令行参数。
//!
//! `--shell-classify` / `--shell-intercept` / `--stdin` / `--shell` 是给
//! shell 钩子用的隐藏桥接参数，普通用户不直接使用。

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "octa-term", version, about = "Seamless terminal math via GNU Octave")]
pub struct Cli {
    /// 计算超时秒数（默认 10，可用 OCTA_TIMEOUT 环境变量覆盖）
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<f64>,

    #[arg(long, hide = true)]
    pub shell_classify: bool,

    #[arg(long, hide = true)]
    pub shell_intercept: bool,

    #[arg(long, hide = true)]
    pub shell: Option<String>,

    #[arg(long, hide = true)]
    pub stdin: bool,

    #[command(subcommand)]
    pub command: Option<Command>,

    /// 要计算的表达式（多个参数用空格连接）
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub message: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 查看/清理计算历史
    History(HistoryArgs),
    /// 集成到 fish（Enter 劫持、多行矩阵、自动续行）
    FishInit,
    /// 集成到 bash（单行兜底：command_not_found_handle）
    BashInit,
    /// 集成到 zsh（单行兜底：command_not_found_handler）
    ZshInit,
    /// 移除全部已安装的 octa-term shell hook
    RemoveShellHook,
    /// 常驻服务：毫秒级求值 + 变量跨表达式保留
    Daemon(DaemonArgs),
    /// 内部：以 daemon 身份运行（由 `daemon start` 通过 current_exe 拉起）
    #[command(name = "__daemon", hide = true)]
    DaemonWorker,
}

#[derive(Debug, Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub command: Option<DaemonCommand>,
}

#[derive(Debug, Subcommand)]
pub enum DaemonCommand {
    /// 启动 daemon（本次登录有效）
    Start,
    /// 停止 daemon
    Stop,
    /// 重启 daemon
    Restart,
    /// 查看 daemon 运行状态
    Status,
    /// 查看 daemon 日志（默认最近 30 行）
    Logs(DaemonLogsArgs),
    /// 安装 systemd user unit：登录自启 + 立即启动
    Enable,
    /// 移除 systemd user unit 并停止服务
    Disable,
}

#[derive(Debug, Args)]
pub struct DaemonLogsArgs {
    #[arg(short = 'n', long, default_value_t = 30)]
    pub lines: usize,
}

#[derive(Debug, Args)]
pub struct HistoryArgs {
    #[arg(short, long, default_value_t = 20)]
    pub limit: usize,

    #[arg(long)]
    pub grep: Option<String>,

    /// Unix 秒或 YYYY-MM-DD[ HH:MM]，只显示此时间之后的历史
    #[arg(long)]
    pub since: Option<String>,

    /// 清空全部历史
    #[arg(long)]
    pub clear: bool,
}

pub fn parse() -> Cli {
    Cli::parse()
}

/// 求值超时：--timeout > OCTA_TIMEOUT > 默认 10 秒。
pub fn eval_timeout(cli: &Cli) -> f64 {
    if let Some(seconds) = cli.timeout {
        return seconds.max(0.1);
    }
    if let Ok(raw) = std::env::var("OCTA_TIMEOUT") {
        if let Ok(seconds) = raw.trim().parse::<f64>() {
            return seconds.max(0.1);
        }
    }
    10.0
}
