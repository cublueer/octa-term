# octa-term 规格书（冻结版）

> 终端无感数学计算：按回车瞬间分流「shell 命令 / 数学表达式」，
> 表达式交给**系统 Octave**（`/usr/bin/octave`，不自带、不捆绑）。

## 1. 求值链路

```
终端输入 → shell 钩子 → Rust 分类器（四态退出码）
   0=命令放行 | 1=表达式求值 | 2=括号未闭合→自动续行 | 3=都不是→shell 报错
   ↓ 表达式
黑名单过滤（system/unix/dos/popen/fopen/eval/input/pause/keyboard/
str2func/feval/python/perl/pkg + delete/unlink/rename/save/load 等破坏性
文件操作，发布前审计追加）+ 字符集校验（禁 `!`/反引号/`$`/非 ASCII）
   ↓
daemon 活着 → IPC 毫秒级求值（全局共享一个 Octave 会话，变量跨表达式保留）
daemon 不在 → 冷调用新开进程（~100ms，无状态）
   ↓
原生格式回显（ans = …、矩阵对齐、format 体系原样）；末尾 ; 静默时印
淡色「已求值」；stderr 噪音过滤（execution_exception / X11 / fontconfig）
```

## 2. 交互

- **fish 全功能**：Enter 劫持 + Ctrl+J 字面换行 + 未闭合括号自动续行 +
  `=` 前缀强制表达式
- **bash/zsh**：一期 command_not_found 单行兜底（`'`/`!`/`^` 差异在文档
  给替代写法）；zsh 二期、bash 三期升级 Enter 劫持
- 超时：**10s 默认可配置**（`--timeout` / `OCTA_TIMEOUT`）；冷调用
  SIGKILL 进程，daemon 先 SIGINT（保变量）再强杀重生
- 误判恢复：`=` 前缀强制表达式；反向用显式路径（`./foo`、`/usr/bin/foo`）
  天然进命令分支，不做 `!` 前缀
- daemon 同步标记每次会话随机化（固定串会被 `disp` 输出伪造，实测已修）；
  多行语句的续行提示符 `> ` 在结果前剥除
- **缺二进制递归守卫（用户真机验收发现）**：hook 调用 `octa-term` 失败会
  触发 command_not_found → 再调 `octa-term` → 无限递归。三壳钩子均加
  存在性守卫（fish: `type -q`/`test -x`；bash/zsh: `command -v`），二进制
  不可用时退化为原生报错；`*-init` 时二进制不在 PATH 则**烙入当前可执行
  文件绝对路径**（未安装也能用），否则用裸名走 PATH

## 3. 分类器

主闸门「首词是命令（PATH/内建/显式路径）就放行」；CJK 判自然语言；
数学判定 = 数字/`(`/`[`/`+`/`-`/引号开头、Octave 函数名表（构建期
`help --list` 生成，`det [1 2]` 空格调用靠它识别）、括号配对（自研状态机，
跳过字符串/注释）。

已知取舍（文档明示）：
- `x-y`/`total-cost` 判数学（变量减法优先）；`my-script.sh`/`README.md`
  （`.ext` 结尾）判文件
- `a.b` 判文件（结构体字段用 `=a.b` 强制）；`a.b+1` 判数学
- `my-app/bin`（无扩展名路径）判数学，Octave 报错兜底

发布前审计追加的护栏：
- `(cd /tmp && ls)` 子 shell：内层首词是命令 → 整条交 shell（octave 的
  cd 会真改 daemon 工作目录，实测已修）；`((1+2))` fish 数学内建同理
- `[`/`[[` 计入内建（无 /usr/bin/[ 的系统）
- Octave 行续接 `...`：`1 + 2 + ...` ↵ 自动续行，补完后整体求值；
  裸 `...` 交 shell 报错

## 4. daemon

`start/stop/restart/status/logs/enable/disable`；`enable` 装 systemd user
unit 登录自启；Octave 子进程走 pty + `PS1` 标记同步、`--no-init-file`
隔离；并发串行排队；崩溃自动重生（变量丢、历史在）。

## 5. 持久化

每次计算完成即落盘 + fsync：`{时间, 表达式, 结果, 耗时, 模式, 状态}`
→ `~/.local/state/octa-term/history.db`（SQLite WAL，0600）；冷调用与
daemon 同库；关机不丢；**变量不落盘**。
`octa-term history [--limit N] [--grep S] [--since TS|YYYY-MM-DD] [--clear]`。

## 6. 工程

- 与 `miyu/` 并列的独立 git 仓库，Rust 单二进制
- 模块：`classify/{mod,token,math,safety}`、`eval`、`history`、`paths`、
  `octave_funcs`（build.rs 生成）
- 分类器函数名表构建期从系统 octave `help --list` 生成；无 octave 的
  构建机退化为内嵌兜底名单

## 7. 实施阶段与状态

| 阶段 | 内容 | 状态 |
|---|---|---|
| 1 | 骨架 + 规格 | ✅ |
| 2 | 四态分类器 + 函数名表 + 安全过滤 | ✅（27 单测） |
| 3 | 冷调用求值 + SQLite 历史 + CLI | ✅ |
| 4 | fish/bash/zsh 钩子与安装卸载 | ✅（pty 真机验证，zsh 待用户机验证） |
| 5 | daemon（pty 常驻 + IPC + systemd enable） | ✅（36 单测 + 2 集成测试；enable/disable 待用户机验证） |
| 6 | README 文档 | ✅；**用户最终验收待进行** |

## 8. 已拍板决策（用户确认）

目录名 `octa-term`（独立 git 仓库）；用系统 Octave（11.3.0，不捆绑）；
默认冷调用 + 可选 daemon（start/enable）；只持久化计算历史、变量不落盘；
daemon 内变量跨表达式保留、全局共享一个会话；历史回看命令要；超时 10s；
自动续行一期做；静默求值淡色确认；强制前缀只要 `=`；安全用黑名单；
输出保留原生格式；history clear 一期做，no-history 开关二期。
