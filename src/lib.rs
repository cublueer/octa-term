//! octa-term 库入口：模块树在这里，`main.rs` 只留派发薄壳。
//! 拆成 lib target 是为了让集成测试（tests/）能直接引用 daemon 客户端等
//! 内部能力，而不是只能通过 bin 的私有模块树。

pub mod args;
pub mod classify;
pub mod daemon;
pub mod eval;
pub mod history;
pub mod hooks;
pub mod octave_funcs;
pub mod paths;
