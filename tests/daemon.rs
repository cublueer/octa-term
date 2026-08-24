//! daemon 端到端测试：拉起真 daemon 进程（真 octave pty 会话），验证
//! 求值、变量保留、SIGINT 超时中断、安全拦截与优雅退出。

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use octa_term::daemon::client;
use octa_term::paths::Paths;

fn octave_available() -> bool {
    Command::new("octave")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn test_paths(state: &Path) -> Paths {
    Paths {
        state_dir: state.to_path_buf(),
        history_db: state.join("history.db"),
        config_dir: state.join("config"),
        config_home: state.join("config-home"),
        fish_hook_file: state.join("fish-hook"),
        bash_hook_file: state.join("bash-hook"),
        zsh_hook_file: state.join("zsh-hook"),
    }
}

fn spawn_daemon(state: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_octa-term"))
        .arg("__daemon")
        .env("OCTA_STATE_DIR", state)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon")
}

async fn wait_daemon(paths: &Paths) -> bool {
    for _ in 0..100 {
        if client::ping(paths).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn daemon_eval_roundtrip() {
    if !octave_available() {
        eprintln!("跳过：本机无 octave");
        return;
    }
    let state = tempfile::tempdir().unwrap();
    let paths = test_paths(state.path());
    let mut child = spawn_daemon(state.path());
    assert!(wait_daemon(&paths).await, "daemon 未在 10s 内就绪");
    let limit = Duration::from_secs(10);

    // 基础求值
    let r = client::eval(&paths, "1+1", limit).await.expect("daemon 响应");
    assert_eq!(r.status, "ok");
    assert!(r.output.contains("ans = 2"), "输出: {}", r.output);

    // 变量跨表达式保留（daemon 的核心价值）
    let r1 = client::eval(&paths, "x = 7", limit).await.unwrap();
    assert_eq!(r1.status, "ok", "赋值输出: {}", r1.output);
    let r2 = client::eval(&paths, "x*2", limit).await.unwrap();
    assert!(r2.output.contains("ans = 14"), "输出: {}", r2.output);

    // 超时：SIGINT 中断当前语句但会话存活、变量保留
    let t = client::eval(&paths, "while 1, end", Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(t.status, "timeout", "输出: {}", t.output);
    let r3 = client::eval(&paths, "x", limit).await.unwrap();
    // Octave 对裸变量显示 `x = 7`（不是 ans）
    assert!(
        r3.output.contains("x = 7"),
        "SIGINT 后变量应保留: {}",
        r3.output
    );

    // daemon 侧安全拦截（防御纵深）
    let b = client::eval(&paths, "system('id')", limit).await.unwrap();
    assert_eq!(b.status, "blocked");

    // 多行矩阵：续行提示符 `> ` 应被剥掉
    let m = client::eval(&paths, "[1 2;\n3 4]", limit).await.unwrap();
    assert!(!m.output.starts_with("> "), "输出: {}", m.output);
    assert!(m.output.contains("1   2"), "输出: {}", m.output);

    // 标记伪造：用户打印固定的标记字符串不应破坏会话同步
    let f = client::eval(&paths, "disp('__OCTA_READY__')", limit).await.unwrap();
    assert!(
        f.output.contains("__OCTA_READY__"),
        "打印标记串应原样回显: {}",
        f.output
    );
    let after = client::eval(&paths, "2+2", limit).await.unwrap();
    assert!(
        after.output.contains("ans = 4"),
        "标记伪造后会话应仍同步: {}",
        after.output
    );

    // 优雅退出
    assert!(client::shutdown(&paths).await, "shutdown 请求应送达");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("daemon 未在 shutdown 后退出");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tokio::test]
async fn second_daemon_is_rejected() {
    if !octave_available() {
        eprintln!("跳过：本机无 octave");
        return;
    }
    let state = tempfile::tempdir().unwrap();
    let paths = test_paths(state.path());
    let mut child = spawn_daemon(state.path());
    assert!(wait_daemon(&paths).await);

    // 第二个实例应被 flock 拒绝
    let second = Command::new(env!("CARGO_BIN_EXE_octa-term"))
        .arg("__daemon")
        .env("OCTA_STATE_DIR", state.path())
        .output()
        .unwrap();
    assert!(!second.status.success(), "第二个 daemon 应失败退出");

    client::shutdown(&paths).await;
    let _ = child.wait();
}
