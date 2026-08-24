# octa-term

在终端里直接打数学表达式，回车即得结果——由**系统 GNU Octave** 计算，
shell 命令不受影响。与 `fish`/`bash`/`zsh` 集成，无需进入任何 REPL。

```
> 1+1
ans = 2
> det([1 2; 3 4])
ans = -2
> ls
```

## 安装

依赖：Rust 1.89+、系统里的 `octave`（构建与运行都要）。

```bash
cargo build --release
install -m755 target/release/octa-term ~/.local/bin/
```

## 三分钟上手

```bash
octa-term fish-init        # 集成到 fish（推荐，全功能）；另有 bash-init / zsh-init
octa-term daemon start     # 可选：常驻服务，毫秒级求值 + 变量跨行保留
octa-term daemon enable    # 可选：登录自启（systemd user unit）
```

新开一个 shell 会话，直接输入：

| 输入 | 结果 |
|---|---|
| `1+1` | `ans = 2` |
| `sin(pi/2)` | `ans = 1` |
| `det([1 2; 3 4])` | `ans = -2`（单行） |
| `[1 2 3;` ↵ `4 5 6]` | 未闭合括号**自动续行**，闭合后求值 |
| `x = 7;` | 淡色 `✓ 已求值`（静默） |
| `=ls` | `=` 前缀强制按 Octave 求值 |

## 工作原理

终端输入按回车瞬间被一个**四态分类器**（Rust，毫秒级）分流：

```
0 像 shell 命令 → 原样执行（PATH/内建/显式路径）
1 像数学表达式 → 安全过滤 → Octave 求值 → 原生格式回显
2 括号未闭合   → 自动换行续打（fish）
3 都不是       → 让 shell 自己报 command not found
```

求值两条路（自动切换）：

- **冷调用**（默认）：每个表达式一个独立 octave 进程，~100ms，无状态；
- **daemon**（`daemon start` 后）：常驻 octave 会话（pty 承载），2–3ms，
  **变量跨表达式保留**——`a = 5` 之后 `a*3` 直接得 `ans = 15`。

每次计算完成即写入历史库 `~/.local/state/octa-term/history.db`
（SQLite WAL，权限 0600，断电不丢）。回看：

```bash
octa-term history                 # 最近 20 条
octa-term history --grep det      # 过滤
octa-term history --since 2026-08-20  # 支持 YYYY-MM-DD / Unix 秒
octa-term history --clear         # 清空
```

## shell 集成细节

| 能力 | fish | bash | zsh |
|---|---|---|---|
| Enter 劫持 + 四态分类 | ✅ | ❌（二期） | ❌（三期） |
| 多行矩阵 / 自动续行 | ✅（Ctrl+J 也可手动换行） | ❌ 单行 | ❌ 单行 |
| 未知命令兜底（command_not_found） | ✅ | ✅ | ✅ |
| `=` 强制表达式 | ✅ | ✅ | ✅ |

**bash/zsh 的历史展开差异**（单行路线绕不开，给替代写法）：

| 想算 | bash/zsh 里写 |
|---|---|
| `2^3` | `2**3` 或 `power(2,3)`（`^` 被 zsh 历史替换吞掉） |
| `[1 2]'`（转置） | `[1 2].'`（引号未闭合进不了钩子） |
| `x != 2` | `x ~= 2` |

fish 路线零妥协，以上都不需要。

卸载：`octa-term remove-shell-hook`（精确摘除 rc 里的标记块，
不会破坏你原有的 `.bashrc`/`.zshrc`）。

### 排障：hook 装了但输入没反应 / 满屏 Unknown command

`*-init` 安装 hook 时会把二进制位置烙进钩子：**PATH 里找得到 `octa-term`
就用裸名（升级省心），找不到就烙入当前可执行文件的绝对路径**——从 build
目录直接 `fish-init` 也能立即工作。若烙入的文件后来被删/移动，钩子的
存在性守卫会让输入退化为原生行为（逐行报 command not found，绝不递归）。
解决：

```bash
install -m755 target/release/octa-term ~/.local/bin/   # 推荐：正式安装
octa-term fish-init                                    # 重装 hook 后新开会话
```

## daemon 管理

```bash
octa-term daemon start|stop|restart|status|logs
octa-term daemon enable|disable   # systemd user unit 登录自启
```

- 超时（默认 10s，`--timeout` / `OCTA_TIMEOUT` 覆盖）：先 `SIGINT` 中断
  当前语句（**变量保留**），3 秒没回来才强杀重生（变量丢、历史不丢）；
- octave 崩溃自动重生；daemon 不在时客户端静默回退冷调用；
- 多终端共享同一个 Octave 会话（变量全局可见），请求串行排队。

## 安全

octa-term 拦截的是你在终端里随便打的字，而 Octave 有能力执行外部命令，
所以表达式进 Octave 前有两道闸（本地 + daemon 各查一遍）：

1. **字符集**：只允许数学字符，`!`（外壳转义）、反引号、`$`、非 ASCII 一律拒绝；
2. **函数黑名单**：执行类（`system`/`unix`/`eval`/`feval`/`str2func`/
   `python`/`perl`/`pkg`…）与破坏性文件操作（`delete`/`unlink`/`rename`/
   `save`/`load`/`movefile`…）整词拦截——文件操作请走 shell，不走表达式。

被拦截的表达式记录为 `⊘` 状态，不会进入 Octave。daemon 会话用
`--no-init-file` 隔离你的 `~/.octaverc`。

## 已知取舍（设计决定，见 specs.md）

- 分类器**数学优先**：`x-y` 判减法而非文件名；`my-script.sh`/`README.md`
  （`.ext` 结尾）判文件交给 shell；`a.b` 判文件（结构体字段用 `=a.b`）；
- 裸标识符（`x`、`pi`）按数学处理：冷调用下未定义会报
  `'x' undefined`，daemon 下就是变量求值；
- 冷调用模式变量不保留（daemon 才保留），这是刻意为之。

## 目录与文件

| 路径 | 内容 |
|---|---|
| `~/.local/state/octa-term/history.db` | 计算历史（0600） |
| `~/.local/state/octa-term/daemon.{sock,pid,lock,log}` | daemon 运行时 |
| `~/.config/octa-term/{bash-hook.sh,zsh-hook.zsh}` | bash/zsh 钩子 |
| `~/.config/fish/conf.d/octa-term.fish` | fish 钩子 |
| `~/.config/systemd/user/octa-term.service` | `daemon enable` 后 |

测试隔离可用 `OCTA_STATE_DIR` / `OCTA_CONFIG_DIR` / `OCTA_FISH_HOOK_FILE`
覆盖上述位置。

## 开发

```bash
cargo test                     # 36 单测 + 2 daemon 集成测试（需要 octave）
```

模块：`classify/`（分类器/安全）、`eval.rs`（冷调用）、`history.rs`（SQLite）、
`hooks/`（三 shell 钩子）、`daemon/`（pty 会话/IPC/控制）。分类器的 Octave
函数名表由 `build.rs` 从 `help --list` 生成，无 octave 的构建机退化为内嵌
名单。规格与决策记录见 `specs.md`。

License: MIT
