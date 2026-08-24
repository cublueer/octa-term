//! 应用路径：XDG 规范下 state 放历史库，config 放钩子文件。
//!
//! 测试可用 `OCTA_STATE_DIR` / `OCTA_CONFIG_DIR` / `OCTA_FISH_HOOK_FILE`
//! 覆盖默认位置，避免污染真实用户数据。

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Paths {    /// 历史库所在目录（~/.local/state/octa-term，权限 0700）
    pub state_dir: PathBuf,
    /// 历史数据库（权限 0600）
    pub history_db: PathBuf,
    /// 配置目录（~/.config/octa-term）
    pub config_dir: PathBuf,
    /// 用户配置根（~/.config，systemd user unit 放这里）
    pub config_home: PathBuf,
    /// fish 钩子：conf.d 目录由 fish 自动加载
    pub fish_hook_file: PathBuf,
    /// bash 钩子（由 ~/.bashrc 中的标记块 source）
    pub bash_hook_file: PathBuf,
    /// zsh 钩子（由 ~/.zshrc 中的标记块 source）
    pub zsh_hook_file: PathBuf,
}

impl Paths {
    pub fn new() -> Paths {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let state_home = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| home.join(".local/state"));
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| home.join(".config"));

        let state_dir = env_override("OCTA_STATE_DIR")
            .unwrap_or_else(|| state_home.join("octa-term"));
        let config_dir = env_override("OCTA_CONFIG_DIR")
            .unwrap_or_else(|| config_home.join("octa-term"));
        let fish_hook_file = env_override("OCTA_FISH_HOOK_FILE")
            .unwrap_or_else(|| config_home.join("fish/conf.d/octa-term.fish"));

        Paths {
            history_db: state_dir.join("history.db"),
            state_dir,
            bash_hook_file: config_dir.join("bash-hook.sh"),
            zsh_hook_file: config_dir.join("zsh-hook.zsh"),
            fish_hook_file,
            config_dir,
            config_home,
        }
    }

    /// daemon IPC socket（~/.local/state/octa-term/daemon.sock）
    pub fn socket_path(&self) -> PathBuf {
        self.state_dir.join("daemon.sock")
    }

    /// daemon pid 文件
    pub fn pid_path(&self) -> PathBuf {
        self.state_dir.join("daemon.pid")
    }

    /// daemon 单实例锁文件
    pub fn lock_path(&self) -> PathBuf {
        self.state_dir.join("daemon.lock")
    }

    /// daemon 日志文件
    pub fn daemon_log(&self) -> PathBuf {
        self.state_dir.join("daemon.log")
    }

    /// 建 state 目录并收紧权限（0700）；历史库文件本体在 history 模块里以
    /// 0600 创建。
    pub fn ensure_state_dir(&self) -> anyhow::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.state_dir)?;
        Ok(())
    }
}

fn env_override(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

impl Default for Paths {
    fn default() -> Self {
        Paths::new()
    }
}
