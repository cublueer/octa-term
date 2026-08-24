//! octa-term：终端无感数学计算。
//!
//! 普通用法：`octa-term 'det([1 2; 3 4])'`
//! 隐藏桥接参数（shell 钩子使用）：
//!   --shell-classify   四态分类，退出码 0=命令 1=数学 2=未闭合 3=其他
//!   --shell-intercept  分类后按数学求值（0=已求值，1=不是数学）

use std::io::Read;
use std::time::Duration;

use anyhow::Result;
use octa_term::classify::Verdict;
use octa_term::daemon;
use octa_term::{args, classify, eval, history, hooks, paths};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("错误: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = args::parse();
    let paths = paths::Paths::new();
    let shell_name = cli.shell.as_deref().unwrap_or("fish");

    // —— 桥接：分类（不落库、不连 daemon、不初始化 Octave，毫秒级）——
    if cli.shell_classify {
        let input = read_input(cli.stdin, &cli.message)?;
        let verdict = classify::classify(&input, shell_name);
        std::process::exit(verdict.exit_code());
    }

    // —— 桥接：拦截求值（fish Enter / bash·zsh command_not_found）——
    if cli.shell_intercept {
        let input = read_input(cli.stdin, &cli.message)?;
        if classify::classify(&input, shell_name) == Verdict::Math {
            // `=` 前缀强制表达式：求值前剥掉（分类器用它跳过命令闸门）
            let expr = input
                .trim()
                .strip_prefix('=')
                .map(str::trim)
                .unwrap_or_else(|| input.trim());
            run_eval(&paths, expr, Duration::from_secs_f64(args::eval_timeout(&cli))).await?;
            std::process::exit(0);
        }
        std::process::exit(1); // 不是数学：交给 shell 报错
    }

    // —— daemon 本体（由 `daemon start` 以当前可执行文件拉起）——
    if matches!(cli.command, Some(args::Command::DaemonWorker)) {
        return daemon::run(&paths).await;
    }

    // —— 子命令 ——
    if let Some(command) = &cli.command {
        match command {
            args::Command::History(history_args) => return run_history(&paths, history_args),
            args::Command::FishInit => return hooks::fish::install(&paths),
            args::Command::BashInit => return hooks::bash::install(&paths),
            args::Command::ZshInit => return hooks::zsh::install(&paths),
            args::Command::RemoveShellHook => return hooks::remove_all(&paths),
            args::Command::Daemon(daemon_args) => {
                return daemon::control::run(&paths, daemon_args).await
            }
            args::Command::DaemonWorker => unreachable!(),
        }
    }

    // —— 直接求值：octa-term '<表达式>' ——
    let input = read_input(cli.stdin, &cli.message)?;
    if input.trim().is_empty() {
        eprintln!("用法: octa-term '<表达式>'   例如: octa-term 'det([1 2; 3 4])'");
        eprintln!("      octa-term history         查看计算历史");
        eprintln!("      octa-term daemon start    启动常驻服务（毫秒级求值+变量保留）");
        std::process::exit(2);
    }
    run_eval(&paths, &input, Duration::from_secs_f64(args::eval_timeout(&cli))).await
}

/// 读取输入：--stdin 时从标准输入读全部，否则把位置参数用空格连接。
fn read_input(use_stdin: bool, message: &[String]) -> Result<String> {
    if use_stdin {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input)?;
        Ok(input)
    } else {
        Ok(message.join(" "))
    }
}

/// 安全过滤 → 远程优先求值（daemon 不在则冷调用）→ 打印 → 落库。
async fn run_eval(paths: &paths::Paths, expr: &str, limit: Duration) -> Result<()> {
    let now = chrono::Utc::now().timestamp();

    // 安全过滤在本地执行：拦截的表达式既不进 daemon 也不进 Octave
    if let Err(reason) = classify::safety::check(expr) {
        eprintln!("⚠ 已拦截：{reason}");
        if let Ok(db) = history::HistoryDb::open(&paths.history_db) {
            let _ = db.insert(
                now,
                expr,
                &format!("<blocked: {reason}>"),
                Some(0),
                history::MODE_COLD,
                history::STATUS_BLOCKED,
            );
        }
        return Ok(());
    }

    // 远程优先：daemon 活着就毫秒级求值，否则冷调用兜底
    let (outcome, mode) = match daemon::client::eval(paths, expr, limit).await {
        Some(response) => (response.into_outcome(), history::MODE_DAEMON),
        None => (
            eval::evaluate(expr, limit).await?,
            history::MODE_COLD,
        ),
    };

    eval::print_outcome(&outcome);

    if let Ok(db) = history::HistoryDb::open(&paths.history_db) {
        let result = match outcome.status {
            eval::Status::Blocked => "<blocked>".to_string(),
            eval::Status::Timeout => "<timeout>".to_string(),
            eval::Status::Ok if outcome.stdout.trim().is_empty() => "<silent>".to_string(),
            _ => outcome.stdout.trim().to_string(),
        };
        let status = match outcome.status {
            eval::Status::Ok => history::STATUS_OK,
            eval::Status::Error => history::STATUS_ERROR,
            eval::Status::Timeout => history::STATUS_TIMEOUT,
            eval::Status::Blocked => history::STATUS_BLOCKED,
        };
        let _ = db.insert(
            now,
            expr,
            &result,
            Some(outcome.duration_ms as i64),
            mode,
            status,
        );
    }
    Ok(())
}

fn run_history(paths: &paths::Paths, args: &args::HistoryArgs) -> Result<()> {
    let db = history::HistoryDb::open(&paths.history_db)?;
    if args.clear {
        let count = db.clear()?;
        println!("已清空 {count} 条历史记录");
        return Ok(());
    }
    let since = args
        .since
        .as_deref()
        .map(parse_since)
        .transpose()?
        .unwrap_or(0);
    let rows = db.list(args.limit, args.grep.as_deref(), since)?;
    if rows.is_empty() {
        println!("（暂无历史）");
        return Ok(());
    }
    for row in rows.iter().rev() {
        let time = chrono::DateTime::from_timestamp(row.ts, 0)
            .map(|dt| dt.format("%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| row.ts.to_string());
        let mut result = row.result.replace('\n', " ");
        if result.chars().count() > 60 {
            result = result.chars().take(60).collect::<String>() + "…";
        }
        let mode = if row.mode == history::MODE_DAEMON { "d" } else { "c" };
        let status = match row.status.as_str() {
            history::STATUS_ERROR => "✗",
            history::STATUS_TIMEOUT => "⏱",
            history::STATUS_BLOCKED => "⊘",
            _ => " ",
        };
        println!(
            "{} {time} [{mode}{status}] {} = {} ({duration}ms)",
            row.id,
            row.expr,
            result.trim(),
            duration = row.duration_ms,
        );
    }
    Ok(())
}

/// --since 解析：纯数字按 Unix 秒；否则按 YYYY-MM-DD[ HH:MM]（本地时区）。
fn parse_since(raw: &str) -> Result<i64> {
    use chrono::offset::LocalResult;
    if let Ok(ts) = raw.trim().parse::<i64>() {
        return Ok(ts);
    }
    let trimmed = raw.trim();
    let formats = ["%Y-%m-%d %H:%M", "%Y-%m-%d"];
    for format in formats {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(trimmed, format) {
            let local = dt.and_local_timezone(chrono::Local);
            if let LocalResult::Single(local) = local {
                return Ok(local.timestamp());
            }
        }
    }
    anyhow::bail!("无法解析时间：{raw}（用 Unix 秒或 YYYY-MM-DD[ HH:MM]）")
}
