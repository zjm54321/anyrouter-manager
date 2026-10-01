# AnyRouter Manager 验证规范与职责矩阵

本文档定义 AnyRouter Manager 项目的质量验收标准、模块归属（Claims & Owners）、测试策略、当前已核实证据与运行约束。

---

## 1. 验证矩阵与职责分工 (Claims & Owners)

| 责任角色 (Owner) | 核心职责与测试范围 | 当前已验证证据 (Proven Evidence) | 约束与已知边界 |
| :--- | :--- | :--- | :--- |
| **Environment Fixer** | • Nix 开发环境确定性<br>• 工具链版本与依赖锁定 | • `nix flake check --no-build` 通过；`flake.lock` 锁定 nixpkgs `7a0f122f`。<br>• 工具链版本验证：Rust 1.98.1、Cargo 1.98.0、Node 24.20.0、npm 11.19.0、Python 3.14.7、uv 0.12.17、util-linux 2.42.3、pkg-config、OpenSSL。<br>• `.#browser` pinned binary 146.0.7680.177.5 离线构建通过，本地 CDP JS smoke 通过。<br>• Python 运行环境：`LD_LIBRARY_PATH` (`cc.lib`) + `PLAYWRIGHT_NODEJS_PATH` 修复生效；`cloakbrowser==0.5.11`、`playwright==1.58.0`、`greenlet==3.5.6` import 与 driver start/stop 通过。 | • 上游官方 152 发行版与当前固定包不一致，项目保持固定锁定版本，不自动升级。<br>• binary unfree / nonredistributable 限制保持仅在独立 package 开启。 |
| **Browser Helper Fixer** | • Helper 进程管理与生命周期<br>• 隔离安全性与清理边界 | • Helper 49 项单元及本地测试（含 locked 虚拟环境、native localhost smoke 及模块入口探测）全部通过。<br>• **进程隔离门控已接入并通过**：通过 `unshare --user --map-current-user --pid --fork --kill-child=KILL --mount-proc` 与 `PDEATHSIG` 控制 helper 及其子进程，已验证 namespace 结束会回收其内子进程，包括脱离原 PGID 的测试后代。 | • 针对先前 SPA 页面判定异常完成修复后，本轮实测通过。<br>• 不承诺永久抵御未来上游 DOM 结构突变或新型动态验证码。 |
| **Backend Fixer** | • Rust 后端业务逻辑与状态机<br>• 存储安全性与并发控制 | • 34 项测试全部通过（0 failed, 0 ignored），fmt/clippy/build 通过。<br>• 已验证：登录失败保留旧 active、状态写入失败原子回滚、candidate 15 分钟 TTL (410)、`GET /api/account` 零凭证泄露、Origin/Host 校验、401 隔离、上游 Cookie 过期仍允许网关 key 转发。<br>• 安全诊断配置 `login_diagnostics = false`（默认关闭），开启时仅随 operation 错误返回白名单元数据（见 `backend/src/diagnostics.rs`），不额外落盘日志。 | • 后端生产静态托管对未匹配的静态资源请求返回 404 及 SPA HTML 响应体。<br>• 明文状态 `0600` 不是加密保险箱；密码仅短期驻留内存，未做底层 secure-wipe。 |
| **Gateway Fixer** | • `/v1` 反向代理网关<br>• 报文透传与流式传输 | • Mock 与集成测试通过：Raw Bytes 传输、Query 穿透、HTTP 状态码 1:1 穿透、SSE 双向流式。<br>• 凭据替换与敏感标头清洗（无 root key、无管理 cookie、剥离 hop-by-hop）。<br>• 不跟随 3xx 重定向；未配置 active key 时返回 503。 | • 固定代理至 `https://anyrouter.top`，防范 SSRF，不提供多站点切换。 |
| **Designer (Frontend Fixer)** | • React Vite 前端界面<br>• 交互规范与状态呈现 | • 17 项测试通过，typecheck / build 通过，设计审查通过。<br>• 桌面端 (1280px) 与移动端 (390px) 4 类状态截图已生成 (`/tmp/screenshots`)。<br>• Vite 开发代理 (`/api`, `/v1` -> 8080) 已配置。<br>• 本地 HTTP 集成测试完成：48 请求 188 断言全部通过。 | • 单账号界面设计，明确不提供多账号列表或自动签到配置。 |
| **父代理 (Parent Agent)** | • 真实账号端到端实测验证 | • **已在独立隔离受控环境完成本次实测流程**：<br>1. `login=succeeded`：通过浏览器完成本次防护页加载和登录；<br>2. `balance_available=true`：成功读取原始额度（未计货币，保持 raw unit）；<br>3. `key_count=1`：成功读取现有 Key 1 个；<br>4. `activation=succeeded`：成功激活并原子持久化；<br>5. `key_reveal_verified=true`：双重鉴权查看 Key 原文校验通过；<br>6. `proxy_models_status=200` (`proxy_models_count=16`, `application/json`)：网关代理成功替换 Bearer 凭据并正确拉取模型列表 JSON；<br>7. `refresh=succeeded`：读取余额刷新链路正常；<br>8. `cleanup=true`：测试结束后临时配置和会话文件已删除。 | • 未调用任何付费模型推理接口，未创建新 Key，无自动签到。<br>• 测试环境使用临时 0700 目录与 0600 state，未污染项目代码库；测试完毕后临时配置和会话文件已删除。 |

---

## 2. 关键设计验证用例明细 (Backend Fixer 用例清单)

后端自动化测试（共 34 项用例）全面覆盖关键安全与可靠性边界：

1. `test_login_failure_preserves_active_account`：异步登录失败时，现存 active 账号完整保留，版本号不变。
2. `test_activate_atomic_persistence_failure_rollback`：本地写入 `state` 失败时事务回滚，保留旧 active 账号。
3. `test_candidate_expiration_ttl`：candidate 超过 15 分钟后调用返回 `410 Gone` (`candidate_expired`)。
4. `test_zero_secrets_in_get_account`：`GET /api/account` 在任何生命周期阶段均不含明文 Key、密码、Cookie 或上游原始 HTML。
5. `test_admin_auth_isolation_from_upstream_errors`：上游返回 401/403/500 时，管理接口返回 200，错误包装在 `operation.error`，管理会话不误登出。
6. `test_upstream_cookie_expired_gateway_still_works`：上游 Session Cookie 过期后，激活的 API Key 仍可用于 `/v1` 网关代理，仅 `refresh` 报错 `upstream_session_expired`。
7. `test_helper_process_namespace_and_cleanup`：验证通过 `unshare` 与 `PDEATHSIG` 在子进程 normal exit、timeout、cancel、double-fork setsid 逃逸及 backend 异常退出时的可靠回收。
8. 安全错误码分类与来源标注 (`login_form_unavailable`, `upstream_session_unverified`, `upstream_login_failed`, `helper_input_invalid`, `browser_unavailable`, `upstream_timeout`)。

---

## 3. 安全声明与已知非目标 (Security & Non-Goals)

1. **真实 AnyRouter 线上环境结论**
   - 本次实测证实了当前环境下通过浏览器完成防护页加载与登录、读取用户资料与现有 Key、以及 `/v1/models` 透明代理的可行性。
   - 不保证上游未来 WAF 算法变更、引入图形验证码或 DOM 变更后仍能无感运行。
   - 上游额度数值保留 raw unit，不进行任何法币汇率换算。
   - 不提供多账号池，不提供自动签到。

2. **存储与密码安全边界**
   - **明文状态文件**：`data/account.json` 采用 `0600` 权限保护，属于敏感明文存储，**不是加密保险箱（not encrypted vault）**。
   - **密码处理**：密码仅用于一次性登录并在短期内存中传递给 helper，**未做底层安全内存擦除（not secure-wipe）**。
   - 临时实测使用独立的隔离目录与 0600 权限文件，所有测试凭据均已清除。用户正式使用时需自行在 Web 界面登录。
