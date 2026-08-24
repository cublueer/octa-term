//! daemon IPC：Unix socket + 4 字节 LE 长度前缀 + JSON 帧（协议 v1）。

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// 单帧上限 8 MiB（矩阵结果不会更大）。
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

/// 客户端 → daemon 的控制帧。
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Control {
    /// 求值请求
    Eval { expr: String, timeout_secs: f64 },
    /// 存活探测
    Ping,
    /// 优雅退出
    Shutdown,
}

/// daemon → 客户端的求值响应。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Response {
    pub output: String,
    /// ok | error | timeout | blocked | session_reset
    pub status: String,
    pub duration_ms: u64,
}

pub async fn write_frame<S: AsyncWrite + Unpin>(stream: &mut S, payload: &[u8]) -> std::io::Result<()> {
    let len = payload.len() as u32;
    stream.write_all(&len.to_le_bytes()).await?;
    stream.write_all(payload).await?;
    stream.flush().await
}

pub async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame too large: {len} bytes"),
        ));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_roundtrip() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let payload = br#"{"cmd":"eval","expr":"1+1","timeout_secs":10}"#.to_vec();
        write_frame(&mut client, &payload).await.unwrap();
        let back = read_frame(&mut server).await.unwrap();
        assert_eq!(back, payload);
    }
}
