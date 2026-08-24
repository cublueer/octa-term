//! Octave 内置函数名表。
//!
//! 正常构建时表由 `build.rs` 从系统 octave 的 `help --list` 生成（几百个
//! 内置函数）；构建机没有 octave 时退化为下方兜底名单。表用于分类器的
//! 「首词是 Octave 函数名 → 数学表达式」判定，属于启发式加分项。

#[cfg(not(octa_fallback))]
include!(concat!(env!("OUT_DIR"), "/octave_functions.rs"));

#[cfg(octa_fallback)]
pub static OCTAVE_FUNCTIONS: &[&str] = &[];

/// 兜底名单只在这些入口里展开：排序数组保证二分查找前提。
const FALLBACK: &[&str] = &[
    "abs", "acos", "acosh", "asin", "asinh", "atan", "atan2", "atanh", "ceil",
    "chol", "conj", "cos", "cosh", "cumprod", "cumsum", "det", "diag", "diff",
    "disp", "eig", "exp", "expm", "eye", "factorial", "fix", "floor", "gamma",
    "gcd", "imag", "inv", "isinf", "isnan", "isreal", "length", "linspace",
    "log", "log10", "log2", "logm", "logspace", "lu", "max", "mean", "median",
    "min", "mod", "norm", "numel", "ones", "pinv", "poly", "polyval", "pow2",
    "power", "prod", "qr", "rand", "randn", "rank", "real", "rem", "reshape",
    "roots", "round", "sign", "sin", "sinh", "size", "sort", "sqrt", "sqrtm",
    "std", "sum", "svd", "tan", "tanh", "trace", "transpose", "var", "zeros",
];

/// 排序去重后的完整函数表（生成表 ∪ 兜底表）。
pub fn octave_functions() -> &'static [&'static str] {
    use std::sync::OnceLock;
    static MERGED: OnceLock<Vec<&'static str>> = OnceLock::new();
    MERGED.get_or_init(|| {
        let mut merged: Vec<&'static str> = OCTAVE_FUNCTIONS.to_vec();
        merged.extend(FALLBACK.iter().copied());
        merged.sort_unstable();
        merged.dedup();
        merged
    })
}

/// 名字是否是已知 Octave 内置函数（大小写敏感，与 Octave 语义一致）。
pub fn is_octave_function(name: &str) -> bool {
    octave_functions().binary_search(&name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_deduped() {
        let table = octave_functions();
        let mut sorted = table.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(table, sorted.as_slice());
    }

    #[test]
    fn fallback_covers_common_math() {
        for name in ["sin", "cos", "det", "inv", "eig", "sqrt", "sum", "log"] {
            assert!(is_octave_function(name), "missing fallback fn {name}");
        }
    }
}
