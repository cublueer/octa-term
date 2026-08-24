//! daemon 客户端：shell 钩子与 CLI 的远程求值入口。
//!
//! 语义：连不上（socket 不存在/拒绝/超时）一律返回 `None`，由调用方回退
//! 冷调用——这是「远程优先、本地兜底」的关键，绝不能把 daemon 故障放大成
//! 用户可见错误。

use std::time::Duration;

use tokio::net::UnixStream;

use crate::eval;
use crate::paths::Paths;

/// 远程求值；daemon 不可达返回 None。
pub async fn eval(paths: &Paths, expr: &str, limit: Duration) -> Option<super::ipc::Response> {
    let mut stream = connect(paths).await?;
    let (mut reader, mut writer) = stream.split();
    let request = serde_json::to_vec(&super::ipc::Control::Eval {
        expr: expr.to_string(),
        timeout_secs: limit.as_secs_f64(),
    })
    .ok()?;
    super::ipc::write_frame(&mut writer, &request).await.ok()?;
    // 响应上限 = 求值超时 + SIGINT 宽限 + 网络余量
    let overall = limit + Duration::from_secs(8);
    let payload = tokio::time::timeout(overall, super::ipc::read_frame(&mut reader))
        .await
        .ok()?
        .ok()?;
    serde_json::from_slice(&payload).ok()
}

/// 存活探测。
pub async fn ping(paths: &Paths) -> bool {
    let Some(mut stream) = connect(paths).await else {
        return false;
    };
    let (mut reader, mut writer) = stream.split();
    let Ok(payload) = serde_json::to_vec(&super::ipc::Control::Ping) else {
        return false;
    };
    if super::ipc::write_frame(&mut writer, &payload).await.is_err() {
        return false;
    }
    tokio::time::timeout(Duration::from_secs(2), super::ipc::read_frame(&mut reader))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

/// 优雅退出；返回是否成功送达。
pub async fn shutdown(paths: &Paths) -> bool {
    let Some(mut stream) = connect(paths).await else {
        return false;
    };
    let (mut reader, mut writer) = stream.split();
    let Ok(payload) = serde_json::to_vec(&super::ipc::Control::Shutdown) else {
        return false;
    };
    if super::ipc::write_frame(&mut writer, &payload).await.is_err() {
        return false;
    }
    tokio::time::timeout(Duration::from_secs(3), super::ipc::read_frame(&mut reader))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

/// 短超时连接（daemon 不存在时快速失败，保住钩子的毫秒级体验）。
async fn connect(paths: &Paths) -> Option<UnixStream> {
    tokio::time::timeout(
        Duration::from_millis(150),
        UnixStream::connect(paths.socket_path()),
    )
    .await
    .ok()?
    .ok()
}

impl super::ipc::Response {
    /// 转成统一的 Outcome，复用冷调用链路的打印与历史格式。
    pub fn into_outcome(self) -> eval::Outcome {
        let status = match self.status.as_str() {
            "timeout" => eval::Status::Timeout,
            "blocked" => eval::Status::Blocked,
            "session_reset" => eval::Status::Error,
            "error" => eval::Status::Error,
            _ => eval::Status::Ok,
        };
        eval::Outcome {
            stdout: self.output,
            stderr: String::new(),
            status,
            duration_ms: self.duration_ms as u128,
        }
    }
}
