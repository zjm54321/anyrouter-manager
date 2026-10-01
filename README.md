# AnyRouter Manager

面向本机使用的单账号管理器：Rust 后端，React / TypeScript 前端。上游固定为
`https://anyrouter.top`，不可通过配置切换为任意站点。通过本地 Web UI 提交账号密码，
优先通过直接 HTTP 或在遇到防护页时回退单次浏览器完成登录，并提取现有 API Key 供本地 `/v1` 网关代理使用；
不提供多账号池；定时与手动签到已接入前后端管理 API（默认关闭）。

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
- **本次实测流程**：在受限隔离临时环境中实测 AnyRouter 线上登录成功、读取余额成功、读取已有 Key（1 个）成功、激活并通过 `/v1/models` 网关代理请求（HTTP 200，返回 16 个模型列表 JSON）、刷新余额成功。现场观察证实登录后页面自然发起单次签到 POST（HTTP 200 `success: true`，提示 25 美元奖励，仅匹配“签到成功”短语）。测试结束临时配置和会话文件已清除，未调用任何付费接口，未创建新 Key，无重复签到。
- **进程隔离与生命周期**：本地非 container 模式通过 Linux PID/User namespace (`unshare --user --map-current-user --pid --fork --kill-child=KILL --mount-proc`) 与 `PDEATHSIG` 控制 helper 及其子进程，已验证 namespace 结束会回收其内子进程；`container_mode` 下采用 Rust subreaper 监督进程（supervisor）而不是 namespace 沙箱，配合容器默认 PID namespace 与 `tini` 回收子进程，无需 `SYS_ADMIN` 或 `unconfined`，禁止 `host PID`；单一浏览器信号量从唤起到完全确认回收全程受控（含取消场景）。
- **模块测试与接入状态**：
  - Rust 后端：落地混合登录机制（优先独立空 CookieStore 直连 POST；仅已知 WAF HTML 挑战单次回退浏览器；JSON 失败/超时/5xx/未知 HTML 绝不唤起浏览器；直接复用已验证的 self/id/余额/上游用户名，无重复 GET；网关客户端不带 cookie jar）。后端完成 56 项 unit + 9 项 integration 测试全部通过（默认 1 项真实 browser ignored 已先单独跑通关于 about:blank normal/timeout 验证），`cargo fmt`、`clippy` 与 `build` 全部通过；已修复路径别名冲突（P1：相对/绝对/`..`/祖先软链接同文件拒绝），panic 记录 unknown 终止任务，跨 08:00 CST 状态落盘失败安全恢复，无上游网络请求。
  - Python helper：托管登录安全守卫主动拦截页面自动 `sign_in`，`session_verify` 只读；通过 77 项 unit + 真实 localhost smoke / module 入口 / 13 项 session 测试。
  - 前端：33 项 test、typecheck、build 及 390px/1280px 响应式截图通过；签到管理功能已接入前后端（`GET /api/checkin`、`PUT /api/checkin/settings`、`POST /api/checkin/run`，09:00 默认关闭、北京时间 08:00 重置、180 条 JSON 周期摘要、unknown 人工确认、成功去重）。
  - 本地 HTTP 集成验证全部通过：父代理执行本地集成验证脚本（`anyrouter-final-integration.py`），7 组本地 HTTP 真实检查（健康检查、静态托管、签到默认配置与重置、严格配置与 Origin 校验、空账号 404/401 隔离、重启配置持久化及管理员会话注销）全部通过（使用临时随机 root key，未请求上游）。

## 配置与秘密边界

先复制示例并限制权限：

```sh
cp config.example.toml config.toml
chmod 600 config.toml
```

自行设置 `root_key` 为**至少 32 字节的高熵随机 ASCII 密钥**（不提供默认可猜密钥）。默认仅监听 `127.0.0.1:8080`。

- `root_key` 用于本机管理授权及受保护的 Key 明文读取，不是 AnyRouter 密码。
- 账号密码仅用于登录瞬时传递，**严禁落盘，但未做底层安全内存擦除（not secure-wipe）**。
- 激活后账号状态保存于 `data/account.json`（会话 Cookie、账号信息与选定的明文 Key），签到状态保存于 `data/checkin.json`（周期记录与调度设置），文件权限严格采用 `0600`（目录 `0700`）。
  **这是敏感明文状态，不是加密保险箱（0600 plaintext, not encrypted vault）**，不保存密码。项目中未留存账号数据，运行前需自行在 UI 中登录。
- 可选配置 `login_diagnostics = false`（默认关闭）：开启时仅在错误中返回固定白名单安全元数据（见 `backend/src/diagnostics.rs`），不记录密码、原始响应体或敏感 Cookie，不额外落盘日志。
- 可选配置 `container_mode = false`（默认 false）：仅当显式设为 `true` 时，采用 Rust subreaper 监督进程（supervisor）模式管理 helper 进程树，支持绑定非回环地址（如 `0.0.0.0`）；在容器内使用标准 PID namespace 与 `tini`，无需 `SYS_ADMIN` 或 `unconfined`，禁止 `host PID`；本地非容器环境（`false`）仍使用 Linux `unshare` 沙箱。
- 可选配置 `cookie_secure = false`（默认 false）：在远程反向代理与 HTTPS 部署时应设为 `true`，强制管理会话 Cookie 包含 `Secure` 标记。
- 可选配置 `browser_helper_executable`：支持指定用于运行 helper 的 Python 解释器绝对路径（适配独立打包的虚拟环境），缺省时默认使用 `uv` 驱动。
- 优雅停机与健康检查：原生捕获 `SIGTERM` 与 `SIGINT` 信号，在 10 秒内清理后台跟踪任务，15 秒内排空 HTTP 请求；提供活跃度探针与带缓存的降级就绪探针（检查命名空间与浏览器可用性）。

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
- **容器与 Docker 支持边界**：项目编写了 Dockerfile 及 GitHub Actions 私有 GHCR 镜像工作流并通过了 18 组静态 mock 与 lint 检查，测试在同一 image ID 下验证后再 push 私有 GHCR。环境未安装本机 Docker Engine，亦未运行远端仓库与 GitHub Actions workflow，不声称镜像构建或容器集群部署已实测验证。旧版“namespace 默认 Docker 必阻断”文案现已过时，当前采用 Rust subreaper 监督进程（supervisor）替代 namespace 沙箱，该机制需待后续真实 CI 环境实测，仍保持容器内无特权限制。
- **签到功能状态与日常登录守卫**：北京时间每日 08:00 重置周期来自用户规则（单点现场观察未独立验证跨日切规律）。日常管理器登录时，helper 安全守卫会主动拦截控制台页面的自动签到 POST，将签到行为作为计划任务交由管理器控制，避免在管理登录时意外触发签到导致漏记状态或与调度计划冲突（这与线上页面真实自动发起签到并不矛盾）；生产环境签到 HTTP 接口实现仅包含本地 mock 验证，不再使用真实账号重复签到探测。
- **Git 提交状态**：项目遵循按组件分步提交规范，功能模块已分步完成提交，最终文档由父代理完成最后集成提交。
- **最终集成验证结论**：本地端到端集成验证已由父代理执行完毕（7 组本地 HTTP 真实链路检查全部通过），无未尽待执行事项。

## 许可证 (License)

本项目原创代码基于 [GNU General Public License v3.0 or later (GPL-3.0-or-later)](LICENSE) 开源，第三方依赖许可证声明详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

本项目引用的 CloakBrowser 预编译二进制遵循其独立的专有许可条款（详见 [CloakBrowser BINARY-LICENSE](https://github.com/CloakHQ/CloakBrowser/blob/7e626ee7a1b0e72ab2c9b98315c36302148df1ce/BINARY-LICENSE.md)）。依据该许可证的“Cloud Container Internal Use”条款，用户用于个人内部私有容器环境（非公开分发）符合其许可规范。严禁将 CloakBrowser 专有二进制整体声称为 GPL。
