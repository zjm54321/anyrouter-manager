# AnyRouter Manager 验证规范与职责矩阵

本文档定义 AnyRouter Manager 项目的质量验收标准、模块归属（Claims & Owners）、测试策略、当前已核实证据与运行约束。

---

## 1. 验证矩阵与职责分工 (Claims & Owners)

| 责任角色 (Owner) | 核心职责与测试范围 | 当前已验证证据 (Proven Evidence) | 约束与已知边界 |
| :--- | :--- | :--- | :--- |
| **Environment Fixer** | • Nix 开发环境确定性<br>• 工具链版本与依赖锁定 | • `nix flake check --no-build` 通过；`flake.lock` 锁定 nixpkgs `7a0f122f`。<br>• 工具链版本验证：Rust 1.98.1、Cargo 1.98.0、Node 24.20.0、npm 11.19.0、Python 3.14.7、uv 0.12.17、util-linux 2.42.3、pkg-config、OpenSSL。<br>• `.#browser` pinned binary 146.0.7680.177.5 离线构建通过，本地 CDP JS smoke 通过。<br>• Python 运行环境：`LD_LIBRARY_PATH` (`cc.lib`) + `PLAYWRIGHT_NODEJS_PATH` 修复生效；`cloakbrowser==0.5.11`、`playwright==1.58.0`、`greenlet==3.5.6` import 与 driver start/stop 通过。 | • 上游官方 152 发行版与当前固定包不一致，项目保持固定锁定版本，不自动升级。<br>• binary unfree / nonredistributable 限制保持仅在独立 package 开启。<br>• CloakBrowser 二进制依据专有许可仅限个人私有容器内部使用，绝非整体 GPL。 |
| **Browser Helper Fixer** | • Helper 进程管理与生命周期<br>• 隔离安全性与清理边界<br>• 会话校验与单浏览器信号量 | • Helper 77 项 unit + 真实 localhost smoke / module 入口 / 13 项 session 测试全部通过（包含历史 49 项基准指标）。<br>• **托管登录安全守卫与只读校验**：托管登录模式下通过浏览器守卫主动拦截页面自动发起的 `POST /api/user/sign_in`（日常管理登录不触发签到副作用）；`session_verify` 模式严格只读，拒绝接收密码，封锁一切写操作与 WebSocket，不执行实际签到。<br>• **进程隔离与生命周期**：本地非 container 模式通过 Linux PID/User namespace (`unshare`) 控制；`container_mode` 下采用 Rust subreaper 监督进程（supervisor）而不是 namespace 沙箱，配合容器默认 PID namespace 与 `tini` 回收子进程，无需 `SYS_ADMIN` 或 `unconfined`，禁止 `host PID`；单一浏览器信号量从唤起到完全确认回收全程受控（含取消场景）。 | • 不承诺永久抵御未来上游 DOM 结构突变或新型动态验证码。<br>• 生产环境签到仅使用轻量 HTTP，不唤起浏览器。 |
| **Backend Fixer** | • Rust 后端业务逻辑与状态机<br>• 存储安全性与并发控制<br>• 混合登录、签到接入与容器适配 | • **后端测试全部通过**：56 项 unit + 9 项 integration 测试全部通过，默认 1 项真实 browser ignored 已先单独跑通（about:blank normal/timeout），`cargo fmt`、`clippy` 与 `build` 通过。<br>• **混合登录已落地**：每次尝试采用独立空 CookieStore 直连 POST `/api/user/login`；仅在识别到已知 WAF 挑战 HTML 时单次回退启动浏览器；遇 JSON 失败/超时/5xx/未知 HTML 绝不启动浏览器；直接复用验证得到的 self/id/balance/username，无重复 GET；密码不落盘，共享网关 HTTP 客户端无 cookie jar；全流程单一截止时间覆盖 auth + self + key 列表。<br>• **安全与故障恢复落地**：已修复路径别名 P1 缺陷（相对、绝对、`..` 及祖先软链接同目标拒绝）；任务 panic 安全记录 unknown 终止 operation；跨 08:00 CST 状态落盘失败安全恢复，重启后将原周期与当前周期标记 unknown 且自动 POST 发起计数为 0；全流程无上游网络请求。<br>• **签到功能已接入**：实现 `GET /api/checkin`、`PUT /api/checkin/settings`、`POST /api/checkin/run`；支持 09:00 默认关闭、北京时间 08:00 重置、180 条 JSON 周期摘要、unknown 人工确认、成功去重。<br>• **运行时配置与容器适配**：`container_mode = false`（显式为 true 时启用 subreaper 监督模式并允许绑定非回环地址）；`cookie_secure = false`（远程 HTTPS 强制 Secure cookie）；`browser_helper_executable`（支持指定 Python 绝对路径）；优雅停机（SIGTERM/SIGINT，10s 任务清理，15s HTTP 排空）；活跃度与带缓存降级就绪探针。 | • 状态存储文件 `0600`（目录 `0700`）为敏感明文，不是加密保险箱；密码仅短期驻留内存，未做底层 secure-wipe。<br>• 本地环境无 Docker Engine，未在真实 Docker 引擎中实测；旧版“namespace 默认 Docker 必阻断”文案现已过时，supervisor 机制需在后续 CI 中实测验证，不删真实限制。<br>• Dockerfile 与 GitHub Actions 私有 GHCR 镜像流通过 18 组静态 mock 与 lint 检查，测试在同一 image ID 下验证后再 push 私有 GHCR；未在远端仓库实际运行 workflow，未声称远端镜像构建或集群可用已实测。 |
| **Gateway Fixer** | • `/v1` 反向代理网关<br>• 报文透传与流式传输 | • Mock 与集成测试通过：Raw Bytes 传输、Query 穿透、HTTP 状态码 1:1 穿透、SSE 双向流式。<br>• 凭据替换与敏感标头清洗（无 root key、无管理 cookie、剥离 hop-by-hop）。<br>• 不跟随 3xx 重定向；未配置 active key 时返回 503。 | • 固定代理至 `https://anyrouter.top`，防范 SSRF，不提供多站点切换。 |
| **Designer (Frontend Fixer)** | • React Vite 前端界面<br>• 交互规范与状态呈现 | • 前端 33 项 test、typecheck、build 以及 390px/1280px 响应式截图通过，设计审查通过。<br>• 签到管理功能已接入前后端（包含设置、手动触发、状态呈现、周期历史列表及确认模态框）。<br>• Vite 开发代理 (`/api`, `/v1` -> 8080) 已配置。<br>• 本地 HTTP 集成测试全部通过。 | • 签到状态严格以服务端下发的 `cycle_date` 为准，前端禁止按客户端自然日推算。 |
| **父代理 (Parent Agent)** | • 真实账号端到端实测验证 | • **已在独立隔离受控环境完成本次实测流程**：<br>1. `login=succeeded`：通过浏览器完成本次防护页加载和登录；<br>2. `balance_available=true`：成功读取原始额度（未计货币，保持 raw unit）；<br>3. `key_count=1`：成功读取现有 Key 1 个；<br>4. `activation=succeeded`：成功激活并原子持久化；<br>5. `key_reveal_verified=true`：双重鉴权查看 Key 原文校验通过；<br>6. `proxy_models_status=200` (`proxy_models_count=16`, `application/json`)：网关代理成功替换 Bearer 凭据并正确拉取模型列表 JSON；<br>7. `refresh=succeeded`：读取余额刷新链路正常；<br>8. `cleanup=true`：测试结束后临时配置和会话文件已清除。<br>• **现场签到事实**：现场观察证实控制台页面在登录成功后自然发起单次 `POST /api/user/sign_in`（HTTP 200 `success: true`，提示 25 美元奖励，仅匹配“签到成功”短语）。<br>• **本地全流程集成验证**：父代理运行本地集成验证脚本（`anyrouter-final-integration.py`）验证成功（ok: true, ready: status ready, static: true, checkin_defaults: true, permissions: true, settings_persisted: true, restart: true, cleanup: true），7 组本地 HTTP 真实检查通过（健康探针、静态托管、签到默认配置与重置、严格配置与 Origin 校验、空账号 404/401 隔离、重启配置持久化及管理员会话注销，临时随机 root key，未请求上游）。 | • 未调用任何付费模型推理接口，未创建新 Key，无重复签到探测。<br>• 生产 HTTP 签到实现仅包含本地 mock 验证，不再使用真实账号重复签到。<br>• 日常管理器登录时守卫主动拦截页面自动签到，将签到控制权交由计划任务，避免漏记状态或与调度冲突（这与线上页面真实自动发起并不矛盾）。<br>• 测试环境使用临时 0700 目录与 0600 state，测试完毕后临时配置和会话文件已清除。 |

---

## 2. 关键设计验证用例明细 (Backend Fixer 用例清单)

后端自动化测试（56 项 unit + 9 项 integration 测试全部通过，默认 1 项真实 browser ignored 已先单独跑通）全面覆盖关键安全与可靠性边界：

1. `test_login_failure_preserves_active_account`：异步登录失败时，现存 active 账号完整保留，版本号不变。
2. `test_activate_atomic_persistence_failure_rollback`：本地写入 `state` 失败时事务回滚，保留旧 active 账号。
3. `test_candidate_expiration_ttl`：candidate 超过 15 分钟后调用返回 `410 Gone` (`candidate_expired`)。
4. `test_zero_secrets_in_get_account`：`GET /api/account` 在任何生命周期阶段均不含明文 Key、密码、Cookie 或上游原始 HTML。
5. `test_admin_auth_isolation_from_upstream_errors`：上游返回 401/403/500 时，管理接口返回 200，错误包装在 `operation.error`，管理会话不误登出。
6. `test_upstream_cookie_expired_gateway_still_works`：上游 Session Cookie 过期后，激活的 API Key 仍可用于 `/v1` 网关代理，仅 `refresh` 报错 `upstream_session_expired`。
7. `test_helper_process_namespace_and_cleanup`：验证通过 `unshare` 或 subreaper supervisor 与 `PDEATHSIG` 在子进程 normal exit、timeout、cancel、double-fork setsid 逃逸及 backend 异常退出时的可靠回收。
8. **混合登录与 CookieStore 隔离测试**：验证每次登录尝试均使用全新的空标准 CookieStore 直接 POST 登录；验证直接返回 JSON 成功时免启动浏览器并直接复用 self/id/balance/username 信息。
9. **浏览器回退门控测试**：验证仅在明确识别到已知 WAF 挑战 HTML 时才回退拉起浏览器 helper；验证遇到 JSON 业务失败、5xx 错误、网络超时或未知 HTML 时绝不启动浏览器。
10. **单浏览器全局信号量与回收确认**：验证信号量从 helper 唤起到进程树完全确认被操作系统回收（包含请求被主动取消场景）的全生命周期持有。
11. **全流程统一登录超时**：验证涵盖认证、self 校验与 token 列表获取的单一截止时间控制，中断与重试不重置截止时间。
12. **优雅停机与任务清理**：验证原生捕获 SIGTERM/SIGINT 信号后停止接收新任务，10 秒内清理后台跟踪任务，15 秒内排空 HTTP 请求并优雅退出。
13. **运行时配置与健康探针**：验证 `container_mode` 网络绑定限制、`cookie_secure` 与 Secure cookie 标记、以及带缓存的降级就绪探针（命名空间与浏览器可用性检查）。
14. 安全错误码分类与来源标注 (`login_form_unavailable`, `upstream_session_unverified`, `upstream_login_failed`, `helper_input_invalid`, `browser_unavailable`, `upstream_timeout`)。
15. **签到周期计算与推进**：验证北京时间 08:00 边界计算，`07:59` 与 `08:01` 分属不同 `cycle_date`，且 `cycle_date` 等价于该时刻的 UTC 日期。
16. **意图优先写**：验证在执行网络 POST 之前，`checkin.json` 中已成功持久化 `running` 状态；模拟写文件失败时，验证上游未收到任何 POST 请求。
17. **服务重启恢复**：模拟进程异常退出留存 `running` 状态，重启后验证记录被自动恢复为 `unknown`，且调度器在当前周期不发起二次自动请求。
18. **跨 08:00 边界超时保护**：模拟跨越 08:00 CST 边界的超时请求，验证其标记为 `unknown` 并锁定关联周期。
19. **未知结果人工重试门控**：当前周期存在 `unknown` 记录时，验证默认手动触发返回 `409 checkin_retry_confirmation_required`；仅在传递 `confirm_retry: true` 时允许执行。
20. **账号隔离与成功去重**：验证 `today` 与 `history` 仅返回当前激活账号的周期记录；当周期内已存在 `success` 时直接返回 `already_recorded: true`。
21. **路径别名防冲突 (P1)**：验证空目录相对/绝对路径、`..` 跃迁及祖先软链接指向同一状态文件时被严格拒绝。
22. **Panic 与跨日切故障安全恢复**：验证任务 panic 时记录 unknown 终止 operation；跨 08:00 CST 状态写失败时保留 running 文件，重启后将原周期与当前周期均标记为 unknown，且自动 POST 发起次数为 0。

---

## 3. 安全声明与已知非目标 (Security & Non-Goals)

1. **真实 AnyRouter 线上环境结论**
   - 本次实测证实了当前环境下通过浏览器完成防护页加载与登录、读取用户资料与现有 Key、以及 `/v1/models` 透明代理的可行性。
   - 现场观察证实登录后控制台页面自然发起单次 `POST /api/user/sign_in`（HTTP 200 `success: true`，提示 25 美元奖励，仅短语匹配）。
   - 日常管理器登录时，helper 守卫主动拦截页面自动签到，将签到行为作为计划任务由管理器显式调度，避免漏记或调度冲突（这与线上真实页面自动发起并不矛盾）。
   - 生产环境签到 HTTP 接口实现仅包含本地 mock 验证，不再使用真实账号重复签到。
   - 不提供多账号池，多账号与保活仅为架构调研（`docs/multi-account-research.md`），无自动探测付费 API。
   - 上游额度数值保留 raw unit，不进行任何法币汇率换算。

2. **存储与密码安全边界**
   - **明文状态文件**：`data/account.json` 与 `data/checkin.json` 采用 `0600` 权限（目录 `0700`），属于敏感明文存储，**不是加密保险箱（not encrypted vault）**，不存储用户密码。
   - **密码处理**：密码仅用于一次性登录并在短期内存中传递给 helper，**未做底层安全内存擦除（not secure-wipe）**。
   - 临时实测使用独立的隔离目录与 0600 权限文件，所有测试凭据均已清除。用户正式使用时需自行在 Web 界面登录。

3. **容器与 Docker 支持边界**
   - Dockerfile 与 GitHub Actions 私有 GHCR 镜像工作流编写完成并通过 18 组静态 mock 与 lint 检查，测试在同一 image ID 下验证后再 push 私有 GHCR。
   - 环境无 Docker Engine，未在真实 Docker 引擎中实测，未在远端仓库运行 workflow，不声称镜像构建与集群部署已实测验证。
   - 旧版“namespace 默认 Docker 必阻断”文案现已过时，当前采用 Rust subreaper 监督进程（supervisor）替代 namespace 沙箱，该机制需在真实 CI 环境实测。

4. **签到功能现状与时区重置约定**
   - 北京时间每日 08:00 重置周期来自用户规则（单点现场观察未独立验证跨日切规律）。
   - 签到管理功能已完成前后端接入：前端 33 项测试通过，支持配置、手动触发、历史列表与确认模态框；后端支持 3 条管理路由与状态机，默认 09:00 计划且关闭。

5. **Helper 测试基准与许可证边界**
   - Helper 77 项 unit + 真实 localhost smoke / module 入口 / 13 项 session 测试通过。
   - 本项目引用的 CloakBrowser 预编译二进制遵循其独立的专有许可条款（依“Cloud Container Internal Use”仅限个人私有容器内部使用），切勿将专有二进制整体声称为 GPL。

6. **Git 提交状态与最终集成验证结论**
   - 项目遵循按组件分步提交规范，功能模块已分步完成提交，最终文档由父代理完成最后集成提交。
   - 本地端到端集成验证已由父代理执行完毕（7 组本地 HTTP 真实链路检查全部通过），无未尽待执行事项。
