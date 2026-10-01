# AnyRouter Manager 验证规范与职责矩阵

本文档定义 AnyRouter Manager 项目的质量验收标准、模块归属（Claims & Owners）、测试策略、当前已核实证据与运行约束。

---

## 1. 验证矩阵与职责分工 (Claims & Owners)

| 责任角色 (Owner) | 核心职责与测试范围 | 当前已验证证据 (Proven Evidence) | 约束与已知边界 |
| :--- | :--- | :--- | :--- |
| **Environment Fixer** | • Nix 开发环境确定性<br>• 工具链版本与依赖锁定 | • `nix flake check --no-build` 通过；`flake.lock` 锁定 nixpkgs `7a0f122f`。<br>• 工具链版本验证：Rust 1.98.1、Cargo 1.98.0、Node 24.20.0、npm 11.19.0、Python 3.14.7、uv 0.12.17、util-linux 2.42.3、pkg-config、OpenSSL。<br>• `.#browser` pinned binary 146.0.7680.177.5 离线构建通过，本地 CDP JS smoke 通过。<br>• Python 运行环境：`LD_LIBRARY_PATH` (`cc.lib`) + `PLAYWRIGHT_NODEJS_PATH` 修复生效；`cloakbrowser==0.5.11`、`playwright==1.58.0`、`greenlet==3.5.6` import 与 driver start/stop 通过。 | • 上游官方 152 发行版与当前固定包不一致，项目保持固定锁定版本，不自动升级。<br>• binary unfree / nonredistributable 限制保持仅在独立 package 开启。<br>• CloakBrowser 二进制依据专有许可仅限个人私有容器内部使用，绝非整体 GPL。 |
| **Browser Helper Fixer** | • Helper 进程管理与生命周期<br>• 隔离安全性与清理边界<br>• 会话校验与单浏览器信号量 | • Helper 82 项 Python + 14 项 native session fixture 测试全部通过。<br>• **托管登录守卫与只读校验**：托管登录模式下通过浏览器守卫主动拦截页面自动发起的 `POST /api/user/sign_in`（日常管理登录不触发签到副作用）；`session_verify` 模式严格只读，拒绝接收密码，封锁一切写操作与 WebSocket，不执行实际签到。<br>• **进程隔离与生命周期**：本地非 container 模式通过 Linux PID/User namespace (`unshare`) 控制；`container_mode` 下采用 Rust subreaper 监督进程（supervisor）而不是 namespace 沙箱，配合容器默认 PID namespace 与 `tini` 回收子进程，无需 `SYS_ADMIN` 或 `unconfined`，禁止 `host PID`；单一浏览器信号量从唤起到完全确认回收全程受控。 | • 不承诺永久抵御未来上游 DOM 结构突变或新型动态验证码。<br>• 生产环境签到仅使用轻量 HTTP，不唤起浏览器。 |
| **Backend Fixer** | • Rust 后端业务逻辑与状态机<br>• 多账号存储、路由与日志<br>• 槽位签到、模型过滤与容器模式 | • **后端测试全部通过**：133 项 unit + 9 项 supervisor integration 测试全部通过（2 项授权 probe 与 1 项原生 browser 测试默认 ignored 此前已单独跑通）；`cargo fmt`、`clippy` 与 `build` 通过。<br>• **多账号与路由切换已落地**：支持 Portfolio V2（最多 64 账号），单生效路由快照，支持无 Key 账号管理与签到但禁止选为路由；Legacy 格式原子迁移保留旧路由；新增账号不自动切换路由；基于 `upstream_user_id` 去重；重登保留原 ID、顺序与路由指向。<br>• **固定时槽签到已落地**：每日固定槽位排程，支持 09:00 默认起跑（默认关闭）、间隔 30 分钟（可通过 Web 配置）；$\text{offset}_{N-1} \ge 1440$ 溢出严格返回 `422 schedule_overflow`；晚到仅 catch-up 当前周期；失败或未知状态当前周期不再自动重发。<br>• **环形缓冲错误日志与系统日志 API 已落地**：网关日志支持 2xx 零正文与非 2xx 捕获前 64 KiB 原始正文（断流记 `truncated: true`）；系统事件日志与全局配置 API 通过本地 HTTP 6 组集成检查。<br>• **模型白名单过滤已落地**：`GET /v1/models` 在 200 OK 且 $\le 2\text{ MiB}$ 时精确过滤 13 个指定模型 ID，其余顺序/元数据保留并重算标头，不拦截客户端推理参数。 | • 状态存储文件 `0600`（目录 `0700`）为敏感明文，不是加密保险箱；密码仅短期驻留内存，未做底层 secure-wipe。<br>• 本地环境无 Docker Engine，未在真实 Docker 引擎中实测；supervisor 机制需在后续 CI 环境实测验证。<br>• 状态文件、签到文件、日志文件与 `config.toml` 均完成实际别名防冲突校验。 |
| **Gateway Fixer** | • `/v1` 反向代理网关<br>• 报文透传与流式传输 | • Mock 与集成测试通过：Raw Bytes 传输、Query 穿透、HTTP 状态码 1:1 穿透、SSE 双向流式。<br>• 请求准入时捕获不可变路由快照，长连接全程绑定初始快照，管理员中途切换路由不影响在途连接。<br>• 凭据替换与敏感标头清洗（无 root key、无管理 cookie、剥离 hop-by-hop）；未配置有效生效路由时返回 503。 | • 固定代理至 `https://anyrouter.top`，防范 SSRF，不提供多站点切换。 |
| **Designer (Frontend Fixer)** | • React Vite 前端界面<br>• 交互规范与状态呈现 | • 前端完成 117 项单元/组件测试、类型检查与构建；包含修复 RevealDialog 身份时序新增的 15 项测试及 17 项 hook race 回归测试。<br>• 完成多账号独立管理面板（`AccountsPanel`）、系统日志双选项卡（`RequestLogPanel`）及原生时间配置组件整装交付。<br>• **原生 UI 真实验收全部通过**：依据报告 `uq3ngeb0`，登录解锁、4 主页面与系统日志导航、6 列数据表、原生仅设置页时间配置、弹窗重开清理、单次刷新（pending 状态与行保留可见、revision/时间更新）及安全登出全部通过；依据报告 `o3wyg0y1`，单次真实登录与保存通过（单一初始文档请求无全页重载，完整阶段流转、密码清理、候选态呈现、upsert 后弹窗关闭，排程/路由/Key/顺序保持不变）。<br>• 错误响应原文在前端 `<pre>` 标签中严格以纯文本渲染，杜绝 DOM 注入与 XSS 风险。 | • 签到状态严格以服务端下发的 `cycle_date` 为准，前端禁止按客户端自然日推算。<br>• 登录保存 pending 过程过快未采样到（由确定性测试覆盖，不作为状态保证）；UI 守卫未观测 helper 上游流量，不伪造上游凭据证明。此前报告 `36ihze3u` 记录的 Modal 重开断言失败作为排查历史客观留存。 |
| **父代理 (Parent Agent)** | • 本地集成脚本验证与历史实测核验 | • **本地全流程集成验证**：父代理执行本地多账号集成脚本（`anyrouter-multi-final-integration.py`）四阶段全部通过；本地 HTTP 6 组系统日志管理检查全部通过。<br>• **真实线上环境验收通过（受控受限）**：两个正式账号在引入 gzip 管理接口解码、父子进程超时 3–5 秒缓冲与公告嵌套 title 补丁后分别独立单次登录通过，账号数由 0→1→2 顺序追加并持久化至 `data/account.json`（0600）；成功读取各 1 个已有 Key（Mask DTO）并完成 200 OK 揭示；显式手选与切换生效路由后，`GET /v1/models` 返回 200 OK 并通过 13 项白名单精确过滤（16 模型过滤出 3 个，0 拒绝），网关请求日志准确映射对应测试账号并成功还原原路由；详见 `docs/live-validation.md`。<br>• **真实刷新与生产签到实测**：依据脱敏报告 `idp7bbvh.json`，两账号串行刷新各 POST 1 次（6724 ms / 5930 ms）成功，原选定 Key、路由与第 3 账号保持不动；依据报告 `tws8rux2.json`，第 2 账号发起当前周期单次 POST 签到成功（2xx `success: true`），状态更新与 operation 保持一致，本轮未触发 `already_done`，未在上游重复执行签到探测。<br>• **早期登录超时与 Modal 失败客观留存**：记录打补丁前登录超时历史（`7tcchj1k.json`）及早期前端 Modal 重开断言失败（`36ihze3u.json`），不掩盖历史缺陷。 | • 未调用付费模型推理接口，未创建新 Key。<br>• 本地服务运行于 18880 端口，测试脚本自动注销管理员会话。<br>• 系统日志与前端整装集成已通过测试与真实 UI 验收。 |
| **CI Fixer** | • 容器构建与自动化工作流 | • 完成 Dockerfile 与 GitHub Actions 私有 GHCR 镜像工作流配置文件编写。<br>• 完成 13 条 pure tests exact 各 1 项通过 + 18 组容器静态 + 6 组 workflow groups (18 event cases) + hadolint/actionlint/shellcheck 检查。<br>• 工作流仅监听私有仓库 `main` 分支 push，无 dispatch 必填参数，无额外 PAT 依赖。 | • 本地主机无 Docker Engine，远端私有 GitHub 仓库与 Actions 实际尚未运行，未在 homelab 或远端执行上传与发布。<br>• 用户全自动交付授权已覆盖手工确认门控，Oracle Gate 1 评审已通过，按阶段推进 Phase 2 与 Phase 3。 |

---

## 2. 关键设计验证用例明细 (Backend Fixer 用例清单)

后端自动化测试（133 项 unit + 9 项 supervisor integration 测试全部通过，2 项授权 probe 与 1 项原生 browser 测试默认 ignored 此前已单独跑通）全面覆盖关键安全与可靠性边界：

1. `test_login_failure_preserves_active_account`：异步登录失败时，现存 active 账号完整保留，版本号不变。
2. `test_activate_atomic_persistence_failure_rollback`：本地写入 `state` 失败时事务回滚，保留旧 active 账号。
3. `test_candidate_expiration_ttl`：candidate 超过 15 分钟后调用返回 `410 Gone` (`candidate_expired`)。
4. `test_zero_secrets_in_get_account`：`GET /api/accounts` 在任何生命周期阶段均不含明文 Key、密码、Cookie 或上游原始 HTML。
5. `test_admin_auth_isolation_from_upstream_errors`：上游返回 401/403/500 时，管理接口返回 200，错误包装在 `operation.error`，管理会话不误登出。
6. `test_upstream_cookie_expired_gateway_still_works`：上游 Session Cookie 过期后，激活的 API Key 仍可用于 `/v1` 网关代理，仅 `refresh` 报错 `upstream_session_expired`。
7. `test_helper_process_namespace_and_cleanup`：验证通过 `unshare` 或 subreaper supervisor 与 `PDEATHSIG` 在子进程 normal exit、timeout、cancel、double-fork setsid 逃逸及 backend 异常退出时的可靠回收。
8. **混合登录与 CookieStore 隔离测试**：验证每次登录尝试均使用全新的空标准 CookieStore 直接 POST 登录；验证直接返回 JSON 成功时免启动浏览器并直接复用 self/id/balance/username 信息。
9. **浏览器回退门控测试**：验证仅在明确识别到已知 WAF 挑战 HTML 时才回退拉起浏览器 helper；验证遇到 JSON 业务失败、5xx 错误、网络超时或未知 HTML 时绝不启动浏览器。
10. **单浏览器全局信号量与回收确认**：验证信号量从 helper 唤起到进程树完全确认被操作系统回收（包含请求被主动取消场景）的全生命周期持有。
11. **全流程统一登录超时**：验证涵盖认证、self 校验与 token 列表获取的单一截止时间控制，中断与重试不重置截止时间。
12. **优雅停机与任务清理**：验证原生捕获 SIGTERM/SIGINT 信号后停止接收新任务，10 秒内清理后台跟踪任务，15 秒内排空 HTTP 请求并优雅退出。
13. **运行时配置与健康探针**：验证 `container_mode` 网络绑定限制、`cookie_secure` 与 Secure cookie 标记、以及带缓存的降级就绪探针（命名空间与浏览器可用性检查）。
14. **安全错误码分类与来源标注**：涵盖 `login_form_unavailable`, `upstream_session_unverified`, `upstream_login_failed`, `helper_input_invalid`, `browser_unavailable`, `upstream_timeout` 等安全脱敏错误。
15. **签到周期计算与推进**：验证北京时间 08:00 边界计算，`07:59` 与 `08:01` 分属不同 `cycle_date`，且 `cycle_date` 等价于该时刻的 UTC 日期。
16. **意图优先写**：验证在执行网络 POST 之前，`checkin.json` 中已成功持久化 `running` 状态；模拟写文件失败时，验证上游未收到任何 POST 请求。
17. **服务重启恢复**：模拟进程异常退出留存 `running` 状态，重启后验证记录被自动恢复为 `unknown`，且调度器在当前周期不发起二次自动请求。
18. **跨 08:00 边界超时保护**：模拟跨越 08:00 CST 边界的超时请求，验证其标记为 `unknown` 并锁定关联周期。
19. **未知结果人工重试门控**：当前周期存在 `unknown` 记录时，验证默认手动触发返回 `409 checkin_retry_confirmation_required`；仅在传递 `confirm_retry: true` 时允许执行。
20. **多账号固定时槽与防溢出排程**：验证按添加顺序分配槽位，验证 $\text{offset}_{N-1} \ge 1440$ 时接口严格拒绝并返回 `422 schedule_overflow`。
21. **路径别名防冲突**：验证空目录相对/绝对路径、`..` 跃迁及祖先软链接指向同一状态文件时被严格拒绝；合法同目录多文件支持。
22. **Panic 与跨日切故障安全恢复**：验证任务 panic 时记录 unknown 终止 operation；跨 08:00 CST 状态写失败时保留 running 文件，重启后将原周期与当前周期均标记为 unknown，且自动 POST 发起次数为 0。
23. **错误日志捕获与截断测试**：验证 2xx 仅记录头部状态，非 2xx 捕获前 64 KiB 原始正文（断流记 `truncated: true`）；验证 1000 条上限旧记录淘汰与 `dropped_count` 递增。
24. **模型白名单 13 精确过滤**：验证 200 OK 且 $\le 2\text{ MiB}$ 时精确过滤 13 个指定 ID，保留其余模型顺序与属性并重算标头。

---

## 3. 安全声明与已知非目标 (Security & Non-Goals)

1. **真实 AnyRouter 线上环境结论**
   - 两个正式账号在受控环境下完成真实登录、读取现有 Key、揭示验证、生效路由手选/切换以及 `/v1/models` 模型精确过滤实测（详见 `docs/live-validation.md`）。
   - 现场观察证实登录后控制台页面自然发起单次 `POST /api/user/sign_in`（HTTP 200 `success: true`，提示 25 美元奖励，仅短语匹配）。
   - 日常管理器登录时，helper 守卫主动拦截页面自动签到，将签到行为作为计划任务由管理器显式调度，避免漏记或调度冲突。
   - 生产环境签到 HTTP 接口实现已完成受控验证（依据脱敏报告 `tws8rux2.json`，第 2 账号发起当前周期单次 POST 签到成功，返回 2xx `success: true`，持久化状态更新为 `success`），未在上游重复执行签到探测。
   - 本轮实测未调用任何付费模型推理接口，未创建新 Token，上游额度保留 raw unit 不做法币换算。

2. **存储与密码安全边界**
   - **明文状态文件**：`data/account.json`、`data/checkin.json` 与 `data/request-logs.json` 采用 `0600` 权限（目录 `0700`），属于敏感明文存储，**不是加密保险箱（not encrypted vault）**，不存储用户登录密码。
   - **密码处理**：密码仅用于一次性登录并在短期内存中传递给 helper，**未做底层安全内存擦除（not secure-wipe）**。
   - 正式使用时需自行在 Web 界面登录。

3. **容器与 Docker 支持边界**
   - Dockerfile 与 GitHub Actions 私有 GHCR 镜像工作流编写完成并通过 13 条 pure tests exact 各 1 项通过 + 18 组静态 mock 与 lint 检查，测试在同一 image ID 下验证后再 push 私有 GHCR。
   - 本地主机无 Docker Engine，未在真实 Docker 引擎中实测，未在远端仓库运行 workflow，不声称镜像构建与集群部署已实测验证。
   - 用户全自动交付授权已覆盖手工确认门控，Oracle Gate 1 评审已通过，按阶段推进 Phase 2 与 Phase 3。

4. **签到功能现状与时区重置约定**
   - 北京时间每日 08:00 重置周期来自用户规则。
   - 固定时槽排程已落地，默认 09:00 起跑（默认关闭），间隔 30 分钟。

5. **Helper 测试基准与许可证边界**
   - Helper 82 项 Python + 14 项 native session fixture 测试通过。
   - 本项目引用的 CloakBrowser 预编译二进制遵循其独立的专有许可条款（依“Cloud Container Internal Use”仅限个人私有容器内部使用），切勿将专有二进制整体声称为 GPL。

6. **Git 提交与模块集成状态**
   - 当前仓库处于基线 12 次提交状态（`main 353282a`），多账号、真实校验记录与日志模块由父代理完成后续集成提交。
   - 前端多账号与系统日志双选项卡已完成整装交付并通过真实 UI 验收（报告 `uq3ngeb0` 与 `o3wyg0y1`）。
   - 系统日志（`docs/system-log-contract.md`）API、配置与持久化已完全实现并通过本地 HTTP 6 组集成检查。
