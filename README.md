# AnyRouter Manager

面向本机使用的单账号管理器：Rust 后端，React / TypeScript 前端。上游固定为
`https://anyrouter.top`，不可通过配置切换为任意站点。通过本地 Web UI 提交账号密码，
通过浏览器完成本次防护页加载和登录并提取现有 API Key，供本地 `/v1` 网关代理使用；
不提供多账号池或自动签到功能。

## NixOS / WSL2 环境

在项目根目录运行：

```sh
nix develop
```

默认 shell 提供 Rust、Cargo、rustfmt、Clippy、Node.js 24（含 npm）、Python、uv、util-linux（含 unshare）、
pkg-config 和 OpenSSL，**不包含、不下载浏览器**。锁定的 nixpkgs 为 `7a0f122f5090cf4c2ade2a13a0e229d4e19ba71f`。

两个 shell 均包含 Python native wheels 所需最小 C++ runtime 的 `LD_LIBRARY_PATH`，
并设置 `PLAYWRIGHT_NODEJS_PATH = "${pkgs.nodejs_24}/bin/node"`。

真实账号登录与自动化浏览器依赖必须使用 browser shell：

```sh
nix develop .#browser
```

此环境提供官方修补版 CloakBrowser（x86_64 固定 **146.0.7680.177.5**，官方 binary 标记为 unfree/nonredistributable，
仅在独立 package 开启允许），设置 `CLOAKBROWSER_BINARY_PATH` 环境变量，无需全局 nix-ld。

**实测与验证状态：**
- **本次实测流程**：在受限隔离临时环境中实测 AnyRouter 线上登录成功、读取余额成功、读取已有 Key（1 个）成功、激活并通过 `/v1/models` 网关代理请求（HTTP 200，返回 16 个模型列表 JSON）、刷新余额成功。测试结束临时配置和会话文件已删除，未调用任何付费接口，未创建新 Key。
- **进程隔离与生命周期已验证**：生产环境通过 Linux PID/User namespace (`unshare --user --map-current-user --pid --fork --kill-child=KILL --mount-proc`) 与 `PDEATHSIG` 控制 helper 及其子进程，已验证 namespace 结束会回收其内子进程，包括脱离原 PGID 的测试后代。
- **自动化测试集全部通过**：Rust 后端 34 项测试通过、Python helper 49 项单元及本地测试通过、前端 17 项测试通过，本地 HTTP 集成 48 请求 188 断言全部通过。

## 配置与秘密边界

先复制示例并限制权限：

```sh
cp config.example.toml config.toml
chmod 600 config.toml
```

自行设置 `root_key` 为**至少 32 字节的高熵随机 ASCII 密钥**（不提供默认可猜密钥）。默认仅监听 `127.0.0.1:8080`。

- `root_key` 用于本机管理授权及受保护的 Key 明文读取，不是 AnyRouter 密码。
- 账号密码仅用于登录瞬时传递，**严禁落盘，但未做底层安全内存擦除（not secure-wipe）**。
- 激活后 `data/account.json` 保存会话 Cookie、账号信息与选定的明文 Key。
  **这是敏感明文状态，不是加密保险箱（0600 plaintext, not encrypted vault）**。测试产生的临时配置和会话文件已删除，项目中未留存账号数据，运行前需自行在 UI 中登录。
- 可选配置 `login_diagnostics = false`（默认关闭）：开启时仅在错误中返回固定白名单安全元数据（见 `backend/src/diagnostics.rs`），不记录密码、原始响应体或敏感 Cookie，不额外落盘日志。

## 一次性安装 helper 依赖与构建

在 browser shell 中：

```sh
uv sync --frozen --project tools/browser-helper
npm --prefix frontend ci
npm --prefix frontend run build
```

## 启动服务

生产式本机运行（Rust 后端直接托管 `frontend/dist` 静态资源）：

```sh
cargo run --locked --manifest-path backend/Cargo.toml -- --config config.toml
```

访问 `http://localhost:8080`，使用配置的 `root_key` 登录后，在界面中输入 AnyRouter 账号密码完成激活。

前端单独开发模式可运行：

```sh
npm --prefix frontend run dev -- --host 127.0.0.1
```

访问 `http://localhost:5173`。Vite 已配置开发代理将 `/api` 与 `/v1` 转发至 `http://127.0.0.1:8080`。

## 验证与已知限制

```sh
nix flake check --no-build
cargo test --locked --manifest-path backend/Cargo.toml
npm --prefix frontend test
```

- 本次实测验证了当前环境下通过浏览器完成防护页加载、登录与既存 Key 读取可用；但不保证上游未来 DOM 变动或防护规则升级后永久有效。
- 额度数值保留上游原始字符串（raw unit），不进行货币汇率猜测。

## 许可证 (License)

本项目原创代码基于 [GNU General Public License v3.0 or later (GPL-3.0-or-later)](LICENSE) 开源，第三方依赖许可证声明详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

本项目引用的 CloakBrowser 预编译二进制遵循其独立的专有许可条款（详见 [CloakBrowser BINARY-LICENSE](https://github.com/CloakHQ/CloakBrowser/blob/7e626ee7a1b0e72ab2c9b98315c36302148df1ce/BINARY-LICENSE.md)）。依据该许可证的“Cloud Container Internal Use”条款，用户用于个人内部私有容器环境（非公开分发）符合其许可规范。
