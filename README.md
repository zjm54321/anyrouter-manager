# AnyRouter Manager

面向本机使用的 AnyRouter 多账号管理器与 API 网关代理：Rust 后端，React / TypeScript 前端。上游固定为 `https://anyrouter.top`，不可通过配置切换为任意站点。

通过本地 Web UI 提交账号密码，优先通过直接 HTTP 或在遇到防护页时回退单次浏览器完成登录，支持管理最多 64 个账号（Portfolio V2）；由管理员显式手选“使用此账号”指定单一全局生效路由，新发起的所有 `/v1` 客户端请求透明代理至该账号，运行中的流式请求（SSE）与长连接保持原有快照不变；系统不提供负载均衡、自动故障切换或保活探测。

多账号固定时槽每日签到已接入前后端接口（默认关闭）；本地环形缓冲错误日志支持保留非 2xx 原始正文供调试；`/v1/models` 接口自动执行 13 个特定模型白名单过滤。

## NixOS / WSL2 环境

在项目根目录运行：

```sh
nix develop
```

默认 shell 提供 Rust、Cargo、rustfmt、Clippy、Node.js 24（含 npm）、Python、uv、util-linux（含 unshare）、pkg-config 和 OpenSSL，**不包含、不下载浏览器**。锁定的 nixpkgs 为 `7a0f122f5090cf4c2ade2a13a0e229d4e19ba71f`。

两个 shell 均包含 Python native wheels 所需最小 C++ runtime 的 `LD_LIBRARY_PATH`，并设置 `PLAYWRIGHT_NODEJS_PATH = "${pkgs.nodejs_24}/bin/node"`。

真实账号登录与自动化浏览器依赖必须使用 browser shell：

```sh
nix develop .#browser
```

此环境提供官方修补版 CloakBrowser（x86_64 固定 **146.0.7680.177.5**，官方 binary 标记为 unfree/nonredistributable，仅在独立 package 开启允许），设置 `CLOAKBROWSER_BINARY_PATH` 环境变量，无需全局 nix-ld。

## 配置与秘密边界

先复制示例并限制权限：

```sh
cp config.example.toml config.toml
chmod 600 config.toml
```

自行设置 `root_key` 为**至少 32 字节的高熵随机 ASCII 密钥**（系统不提供默认密钥，运行前需自行生成并在本地妥善保存，切勿将密钥提交至代码仓库）。默认仅监听 `127.0.0.1:8080`。

- `root_key` 用于本机管理鉴权、`/v1` 网关代理请求以及通过 `Authorization: Bearer <root_key>` 查看选定 Key 明文，不是 AnyRouter 账号密码。
- 账号密码仅用于登录瞬时传递，**严禁落盘，但未做底层安全内存擦除（not secure-wipe）**。
- **状态持久化与文件安全**：
  - 账号列表保存于 `data/account.json`（Portfolio V2 格式，上限 8 MiB，最多 64 账号，支持从单账号旧版 Active 格式完整原子迁移并保留旧路由；新增账号不自动切换路由；基于 `upstream_user_id` 去重；重新登录保留原 ID、顺序与路由状态；未绑定 Key 的账号可签到但不可选为路由）。
  - 签到状态保存于 `data/checkin.json`（上限 8 MiB，每账号最多 180 条历史，系统总上限 11,520 条；旧历史身份可保留并加载；清理时保护当前、上一周期、未来及 running 记录）。
  - 请求日志保存于 `data/request-logs.json`（上限 1 MiB，最多 1000 条记录；内存容量 256 环形缓冲，异步落盘不阻塞流式传输）。
  - 文件权限严格限定为 `0600`（所有父目录必须为 `0700`）。启动时对状态路径、日志路径与 `config.toml` 进行别名与祖先路径冲突校验。
  - **重要声明：这是敏感明文存储，不是加密保险箱（0600 plaintext, not encrypted vault）**，不存储用户登录密码。
- **请求日志与错误保真**：
  - 2xx 成功请求仅记录时间戳、账号标识与 HTTP 状态码（不记录正文，不代表 SSE 流完成）。
  - 非 2xx 错误响应保真记录前 64 KiB 原始正文（不脱敏，断流标记 `truncated: true`；用户明确个人使用保留原文供排错，若上游自身回显秘密则按原文留存；前端在 `<pre>` 中严格以纯文本渲染）。
- **模型白名单清洗**：
  - `GET /v1/models` 在上游返回 200 OK 且体积 $\le 2\text{ MiB}$ 时，精确过滤 13 个指定模型 ID，保留其余模型顺序与属性并重算 `Content-Length`，不拦截客户端推理请求参数。
- **可选运行时配置**：
  - `container_mode = false`（默认 false）：仅当显式设为 `true` 时，采用 Rust subreaper 监督进程（supervisor）模式管理 helper 进程树，支持绑定非回环地址；本地非容器环境（`false`）仍使用 Linux `unshare` 沙箱。
  - `cookie_secure = false`（默认 false）：在远程反向代理与 HTTPS 部署时应设为 `true`。
  - `browser_helper_executable`：支持指定用于运行 helper 的 Python 解释器绝对路径。
- **优雅停机**：原生捕获 `SIGTERM` 与 `SIGINT` 信号，在 10 秒内清理后台跟踪任务，15 秒内排空 HTTP 请求后退出。

## 一次性安装 helper 依赖与构建

在 browser shell 中：

```sh
uv sync --frozen --project tools/browser-helper
npm --prefix frontend ci
npm --prefix frontend run build
```

## 本地启动说明

本地服务已在 18880 端口保持运行，测试环境使用临时随机 `root_key`；正式生产密钥生成与远端部署将在 Oracle Gate 1 评审通过后推进。

生产式本机运行（Rust 后端直接托管 `frontend/dist` 静态资源）：

```sh
cargo run --locked --manifest-path backend/Cargo.toml -- --config config.toml
```

访问 `http://localhost:8080`，使用配置的 `root_key` 登录后，在界面中添加管理账号与配置签到。

前端单独开发模式：

```sh
npm --prefix frontend run dev -- --host 127.0.0.1
```

访问 `http://localhost:5173`。Vite 开发代理将 `/api` 与 `/v1` 转发至 `http://127.0.0.1:8080`。

## 验证与已知边界

```sh
nix flake check --no-build
cargo test --locked --manifest-path backend/Cargo.toml
npm --prefix frontend test
```

- **后端测试状态**：Rust 后端 133 项 unit + 9 项 supervisor integration 测试全部通过（2 项授权 probe 与 1 项原生 browser 测试默认 ignored 此前已单独跑通）。
- **本地集成验证**：父代理运行本地集成脚本（`anyrouter-multi-final-integration.py`）四阶段全部通过；本地 HTTP 6 组系统日志管理检查全部通过。
- **历史与线上实测记录**：受控环境下完成多账号真实登录、读取 Key、揭示验证、生效路由手选/切换以及 `/v1/models` 精确过滤；真实账号刷新（6724/5930 ms）与单次生产签到（2xx `success: true`）实测通过，未在上游重复执行签到探测，未调用付费模型推理。
- **多账号签到固定时槽**：每日按账号添加顺序分配固定时槽，默认起始 09:00（默认关闭），间隔 30 分钟。若排程累计超过 24 小时周期（1440 分钟）严格返回 `422 schedule_overflow`。单账号单周期唯一，失败或未知状态不自动重试，晚到仅 catch-up 当前周期。
- **前端接入现状**：前端 117 项单元/组件测试、类型检查与构建全部通过（包含修复 RevealDialog 身份时序新增的 15 项测试及 17 项 hook race 回归测试）；整装页面通过原生 UI 验收（涵盖 4 主页面与系统日志导航、6 列数据表、原生仅设置页时间配置、弹窗重开清理、单次刷新与真实登录保存）。
- **容器与 CI 状态**：项目完成 Dockerfile 及私有 GHCR 工作流，完成 13 条 pure tests exact 各 1 项通过 + 18 组容器静态 + 6 组 workflow groups (18 event cases) + hadolint/actionlint/shellcheck 检查；本地主机无 Docker，远端 GitHub 仓库与 Actions 实际尚未运行，未在 homelab 或远端执行上传与发布。用户全自动交付授权已覆盖手工确认门控，Oracle Gate 1 评审已通过，按阶段推进后续工作。
- **Git 提交基线**：当前仓库处于基线 12 次提交状态（`main 353282a`），多账号、真实校验记录与日志模块由父代理完成后续集成提交，不预写未知 commit hash 或提交计数。

## 许可证 (License)

本项目原创代码基于 [GNU General Public License v3.0 or later (GPL-3.0-or-later)](LICENSE) 开源，第三方依赖许可证声明详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
