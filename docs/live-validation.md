# AnyRouter Manager 真实线上环境验收记录 (Live Validation Report)

本文档记录 AnyRouter Manager 针对真实 AnyRouter 线上环境执行的受控登录、多账号管理与网关代理实测核验结果。

> **安全与凭据保护准则**：
> 本次验证遵循凭据最小化与保护原则。测试报告与本文档不记录用户账号密码、`root_key`、会话 Cookie、真实用户 ID 或真实 API Key 明文。报告文件存放在受限目录（权限 `0600`）。

---

## 1. 验证背景与问题修复 (Context & Patches)

在针对真实线上环境的早期测试中，两个正式账号均在无头浏览器登录阶段出现超时失败（脱敏报告：`/tmp/opencode/anyrouter-live-report-7tcchj1k.json`）：
- 阶段表现：停留在 `authenticating` 阶段，最终抛出 `upstream_timeout`（错误来源：`helper`），登录失败退出，未进入后续余额、Key 或网关测试流程。

后续开发针对该现象引入了三项防御性补丁：
1. **上游管理接口 gzip 显式解码支持**：避免上游压缩响应导致的数据截断或解析阻塞；
2. **父子进程超时预留缓冲**：在 Rust 父进程统一超时与 Python 子进程之间保留 3–5 秒缓冲时序，避免子进程未来得及退出即被强杀；
3. **弹窗公告嵌套节点与 title 属性定位增强**：增强对上游 DOM 嵌套弹窗及公告标题遮挡的穿透与关闭能力。

**事实定界**：由于上述三项调整随新版本同步生效，**无法孤立证明上游公告遮挡是此前超时的唯一根因**，各项因素可能共同改善了真实环境交互的稳定性。文档保留此项客观事实，不掩盖历史超时记录。

---

## 2. 真实账号实测数据与步骤 (Live Validation Steps)

修复后，对两个正式账号分别进行了独立单次受控验证，两轮实测均顺利通过（登录重试次数均为 0，无额外重发）。

### 2.1 第一账号验收（脱敏报告：`anyrouter-live-report-xuocfmki.json`）

| 步骤 / 阶段 | 接口与状态码 | 执行结果与观察证据 | 账号与路由状态 |
| :--- | :--- | :--- | :--- |
| **异步登录** | `POST /api/accounts/login` (202 Accepted) | 阶段顺序流转：`authenticating` → `reading_account` → `listing_keys` → `done`，状态 `succeeded`。 | 账号总数：0 → 0（Candidate 阶段） |
| **持久化保存** | `POST /api/accounts` (202 Accepted) | 保存成功，上游 `balance`（原始额度数值，不做汇率换算）与 1 个已启用 API Key 成功读取，前端仅暴露 Mask DTO。 | 账号总数：0 → 1（已落盘） |
| **Key 揭示** | `POST /api/accounts/{id}/key/reveal` (200 OK) | 鉴权通过，确认明文 API Key 在内存/受限文件中真实存在（未回显或日志打印）。 | 账号 1 |
| **网关手选路由** | 显式参数 `--allow-initial-route` | 针对原为 `null` 的全局路由，显式将账号 1 指定为生效路由。 | 当前路由：指向账号 1 |
| **模型代理过滤** | `GET /v1/models` (200 OK) | 成功代理至上游，返回 3 个模型；精确排除列表（黑名单 13 项排除规则生效，未命中任何被排除 ID，即 denied ID 计数为 0；前期上游基线观察总数为 16 条，本次脱敏报告证明最终结果为 3 条）。 | 响应正常 |
| **请求日志映射** | 读取 `data/request-logs.json` | 验证 200 OK 日志准确映射并记录该测试账号本地 ID。 | 日志映射一致 |
| **管理退出** | Harness 本地 Session Logout | 测试脚本自动注销管理员会话，不遗留管理鉴权态。 | `logged_out: true` |

### 2.2 第二账号验收（脱敏报告：`anyrouter-live-report-05fzcb7p.json`）

| 步骤 / 阶段 | 接口与状态码 | 执行结果与观察证据 | 账号与路由状态 |
| :--- | :--- | :--- | :--- |
| **异步登录** | `POST /api/accounts/login` (202 Accepted) | 顺序完成 `authenticating` → `reading_account` → `listing_keys` → `done`，状态 `succeeded`。 | 账号总数：1（Candidate） |
| **持久化保存** | `POST /api/accounts` (202 Accepted) | 成功追加保存，读取到 `balance` 与 1 个已启用 API Key（Mask DTO）。 | 账号总数：1 → 2（顺序追加） |
| **Key 揭示** | `POST /api/accounts/{id}/key/reveal` (200 OK) | 验证通过，确认该账号 API Key 存在。 | 账号 2 |
| **路由动态切换** | 显式调用路由切换接口 | 将生效路由切至账号 2，调用 `GET /v1/models` 返回 200 OK，返回 3 个模型（精确排除列表生效，0 拒绝），请求日志准确对应账号 2。 | 路由切至账号 2 |
| **原路由还原** | Harness 路由还原逻辑 | 验证完毕后，原生效路由恢复指向账号 1（`original_route_restored: true`）。 | 路由还原为账号 1 |
| **管理退出** | Harness 本地 Session Logout | 测试脚本注销自身管理员会话。 | `logged_out: true` |

---

## 3. 运行环境与已知边界 (Environment & Boundaries)

1. **服务与状态文件**：
   - 本地 `18880` 端口后台服务保持运行，`config.toml` 配置与 `root_key` 保持一致；
   - 本地 `data/account.json`（权限 `0600`，目录 `0700`）合法持有真实账号的会话凭据与 API Key，敏感明文存储，无密码存储。
2. **刷新与签到真实业务核验**：
   - **真实账号刷新**（报告 `idp7bbvh.json`）：前两个账号串行刷新各发起 1 次 POST（6,724 ms / 5,930 ms）成功，流转完整经历 `reading_account` → `listing_keys` → `done`，更新 revision 与时间戳，已知 challenge 事件后正常回退，原选定 Key、生效路由与第 3 账号保持不动。
   - **生产签到核验**（报告 `tws8rux2.json`）：针对此前 preflight failed 的第 2 账号发起当前周期单次 POST 签到，操作状态 `succeeded`，持久化记录与 operation 状态一致更新为 `success`，生产环境捕获 2xx 且 `success: true`；本轮未触发 `already_done`，未进行二次重复探测。
3. **未执行项（受控保护）**：
   - **付费模型推理未调用**（`paid_inference: "not_run"`），未消耗真实付费额度；
   - **上游新 Token 未创建**（`token_creation: "not_run"`），仅读取已有 Key；
   - **非 2xx 原始错误正文捕获**：由于实测未主动注入故障，非 2xx 错误正文的真实捕获未在线上执行（`raw_error_storage_live_test: "not_run_no_failure_induced"`），该功能已由本地确定性集成测试覆盖。
4. **系统集成与交付状态**：
   - **系统日志（System Logs）模块**：系统日志管理 API、持久化与全局配置已完全实现，通过本地 HTTP 6 组集成检查（settings、filter、clear、restart、perms 等全部通过），并在前端双选项卡完成真实验收；
   - **前端集成验收**：已完成整装交付并通过真实端到端 UI 验收（报告 `uq3ngeb0` 与 `o3wyg0y1`，覆盖页面导航、原生时间设置、弹窗清理、UI 刷新与真实登录保存）；
   - **交付授权与推进**：用户全自动交付授权已覆盖原手工确认门控；只读 Oracle Gate 1 评审已通过，按序进入 Phase 2 私有仓库创建与 GHCR 镜像构建。
