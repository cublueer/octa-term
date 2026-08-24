//! 构建期从系统 Octave 的 `help --list` 生成内置函数名表，供分类器识别
//! 「首词是 Octave 函数」的表达式（如 `det([1 2; 3 4])`）。
//!
//! 找不到 octave（或它异常退出）时退化到内嵌的兜底名单（`octa_fallback` cfg），
//! 保证任何机器上都能编译；函数表只是分类器的加分项，不是正确性前提。

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=OCTA_OFFLINE_FUNCLIST");
    println!("cargo:rustc-check-cfg=cfg(octa_fallback)");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let dest = out_dir.join("octave_functions.rs");

    let names = collect_from_system_octave();
    match names {
        Some(names) if !names.is_empty() => {
            write_table(&dest, &names);
            // 表内容由系统 octave 决定，不在源码里体现；构建缓存仍以
            // build.rs 时间戳驱动即可。
        }
        _ => {
            eprintln!(
                "cargo:warning=octa-term: `octave --eval 'help --list'` failed; \
                 classifier falls back to the embedded function list"
            );
            fs::write(
                &dest,
                "pub static OCTAVE_FUNCTIONS: &[&str] = &[];\n",
            )
            .expect("write fallback table");
            println!("cargo:rustc-cfg=octa_fallback");
        }
    }
}

/// 运行 `octave --no-gui --quiet --no-init-file --eval 'help --list'`，
/// 每行取第一个空白分隔的 token、剥掉 `.oct` 后缀，保留纯标识符。
fn collect_from_system_octave() -> Option<Vec<String>> {
    let output = Command::new("octave")
        .args([
            "--no-gui",
            "--quiet",
            "--no-init-file",
            "--eval",
            "help --list",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut names = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("***") {
            continue;
        }
        for token in line.split_whitespace() {
            let token = token.strip_suffix(".oct").unwrap_or(token);
            if is_identifier(token) {
                names.insert(token.to_string());
            }
        }
    }
    Some(names.into_iter().collect())
}

fn is_identifier(token: &str) -> bool {
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn write_table(dest: &PathBuf, names: &[String]) {
    let mut out = String::with_capacity(names.len() * 24 + 64);
    out.push_str("pub static OCTAVE_FUNCTIONS: &[&str] = &[\n");
    for name in names {
        out.push_str(&format!("    \"{name}\",\n"));
    }
    out.push_str("];\n");
    fs::write(dest, out).expect("write octave function table");
}
