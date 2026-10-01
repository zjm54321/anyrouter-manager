# AnyRouter Manager API 契约与技术规范

本文档定义 AnyRouter Manager 后端与前端之间的接口契约、数据模型、安全规范及网关行为。供后端（Backend Fixer）与前端（Designer / Frontend Fixer）共同遵守。

---

## 1. 架构与技术基准

| 配置项 | 规范要求 |
| :--- | :--- |
| **系统架构** | 单账号模式；Rust 后端（提供管理 API 与 `/v1` 网关）+ React Vite 前端（TypeScript）。不提供多账号池；定时与手动签到管理功能已接入前后端（详见 `docs/checkin-contract.md`）。 |
| **固定上游** | 固定为 `https://anyrouter.top`。禁止通过请求头、查询参数或路径覆盖上游地址（防 SSRF）。 |
| **系统凭证** | 由本地 `config.toml` 配置 `root_key`（至少 32 字节高熵随机 ASCII），统一作为本地管理鉴权 (`/api/admin/*`) 与网关访问鉴权 (`/v1/*`) 的凭证。 |
| **状态持久化与安全边界** | 本地状态文件采用私有权限 `0600`（父目录 `0700`）与原子写，包含账号状态 `data/account.json`（会话 Cookie、账号信息与选中的 API Key）与签到状态 `data/checkin.json`（周期记录与调度设置）。<br>• **注意：这是敏感明文存储，不是加密保险箱（0600 plaintext, not encrypted vault）**，不存储用户密码。<br>• 用户名与密码仅在登录时驻留短期内存供调用，**严禁落盘，但未做底层安全内存擦除（not secure-wipe）**。<br>• 系统无法承诺必然绕过未来上游变更后的 WAF、ESA 或人机挑战。 |
| **服务端口与部署** | 后端默认监听 `127.0.0.1:8080`。<br>• **运行时模式配置**：支持 `container_mode = false`（默认 false，显式设为 true 时方可监听非回环地址并启用 supervisor 监督模式）；`cookie_secure = false`（默认 false，要求远程 HTTPS 反向代理部署时启用以打上 Secure 标记）。<br>• **开发环境**：Vite 监听 `127.0.0.1:5173`，已配置 dev proxy 将 `/api` 与 `/v1` 同源代理至 `127.0.0.1:8080`。<br>• **生产环境**：Rust 后端直接托管 `frontend/dist` 静态资源；对于未匹配的静态资源请求，返回 `404` 状态码及 SPA HTML 页面体（实现行为，不承诺全静态路径 200）。 |
| **管理会话机制** | 使用 `HttpOnly; SameSite=Strict; Path=/api/` Cookie，有效期 8 小时。前端**禁止**将会话凭据放入 `localStorage`、`sessionStorage` 等客户端持久存储。 |
| **安全请求校验** | 后端严格校验所有管理请求的 `Origin` 与 `Host` 头，防止跨站或劫持请求。 |
| **缓存策略** | 所有 `/api/*` 管理接口响应头均强制下发 `Cache-Control: no-store`，禁止任何中间代理与浏览器缓存。 |
| **Cookie 隔离与过期行为** | 来自上游 AnyRouter 的所有 Cookie 仅保存在后端受控状态中，严禁下发给前端或客户端。<br>• **过期加载行为**：若上游 Session Cookie 过期，已激活的 API Key 仍可继续支撑 `/v1` 网关代理请求；仅在触发 `refresh` 时向管理端报错 `upstream_session_expired`。 |
| **数据格式标准** | • 所有实体 ID 必须为字符串 (`string`)。<br>• 时间戳统一使用 ISO 8601 UTC 格式（如 `2026-09-30T12:00:00Z`）。<br>• 额度字段 `quota_raw` 与 `used_quota_raw` 为十进制字符串（如 `"1000000"`），禁止后端或协议层做美元换算。 |
| **优雅停机与任务管理** | 后端原生捕获 `SIGTERM` 与 `SIGINT` 信号；收到信号后停止接收新任务，在 10 秒内清理后台跟踪任务，15 秒内排空存量 HTTP 请求后优雅退出。 |
| **Browser Helper 与混合登录** | 采用混合登录架构（HTTP-First）：优先直接发起 HTTP POST 登录；仅在明确收到已知 WAF 挑战 HTML 时单次回退启动 Browser Helper（遇 5xx/超时/未知 HTML 不回退）。受全局单一浏览器信号量保护（从 spawn 直至进程完全确认回收，含中断与取消）。本地非容器环境通过 Linux namespace 与 `PDEATHSIG` 调用 helper 隔离进程；`container_mode = true` 下采用 Rust subreaper 监督进程（supervisor）模式管理 helper 进程树，配合容器 PID namespace 与 `tini`，无需 `SYS_ADMIN` 或 `unconfined`，禁止 `host PID`。可选配置 `browser_helper_executable` 指定 Python 解释器绝对路径。若在基础 shell 或缺少修补浏览器环境下启动，登录接口返回 `browser_unavailable`。 |

---

## 2. 数据传输对象 (DTO Specifications)

### 2.1 核心状态对象 `AccountState`

管理端核心状态接口 `GET /api/account` 返回的根对象：

```typescript
export interface AccountState {
  active: ActiveAccount | null;
  candidate: CandidateAccount | null;
  operation: BackgroundOperation | null;
}
```

#### 子对象定义

```typescript
export interface ActiveAccount {
  revision: string; // 乐观锁版本号，每次持久化变更递增
  username: string;
  upstream_user_id: string;
  balance: AccountBalance;
  selected_key: SelectedKeySummary;
  activated_at: string; // ISO 8601 UTC
}

export interface CandidateAccount {
  id: string; // candidate_id
  username: string;
  upstream_user_id: string;
  balance: AccountBalance;
  keys: KeySummary[];
  expires_at: string; // ISO 8601 UTC，内存暂存 15 分钟
}

export interface AccountBalance {
  quota_raw: string; // 原始额度数值（字符串格式十进制）
  used_quota_raw: string; // 已用额度数值（字符串格式十进制）
  fetched_at: string; // ISO 8601 UTC
}

export interface KeySummary {
  id: string;
  name: string;
  masked: string; // 脱敏展示，例如 "sk-abc...1234"
}

export interface SelectedKeySummary {
  id: string;
  name: string;
  masked: string;
}

export interface BackgroundOperation {
  id: string; // operation_id
  kind: "login" | "activate" | "refresh" | "checkin";
  status: "running" | "succeeded" | "failed";
  phase:
    | "authenticating"
    | "reading_account"
    | "listing_keys"
    | "reading_key"
    | "committing"
    | "checking_in"
    | "done";
  error: OperationError | null;
}

export interface OperationError {
  code: ErrorCode;
  message: string; // 安全的脱敏文案
  source?: "helper" | "credentials" | "self" | "tokens" | "key" | "persistence";
}
```

### 2.2 轮询规则与安全约束
- **轮询控制**：仅当 `operation.status === "running"` 时，前端以 **1 秒** 为间隔轮询 `GET /api/account`。一旦状态变为 `succeeded` 或 `failed`，立即停止轮询。
- **敏感信息零泄露**：`GET /api/account` 任何时候都不得输出原始 API Key、上游 Cookie、用户密码或上游原始响应体。

---

## 3. 错误处理模型与分类法 (Error Taxonomy)

### 3.1 统一错误结构

所有同步失败响应均遵循标准格式：

```json
{
  "error": {
    "code": "error_code_string",
    "message": "用户可读的安全脱敏描述信息",
    "source": "helper"
  }
}
```

### 3.2 HTTP 状态码映射准则
- **400 Bad Request**：请求体缺失、JSON 解析失败或参数格式校验错误。
- **401 Unauthorized**：**仅限本地管理员会话失效或 root key 鉴权错误**。上游 AnyRouter 的任何错误均不得导致管理 API 返回 401（上游错误必须记录在 `operation.error` 中），防止前端误将本地管理员登出。
- **409 Conflict**：
  - `operation_in_progress`：已有后台任务正在运行，拒绝新操作（单任务并发限制）。
  - `stale_revision`：提交的版本号与当前服务端 `active.revision` 不一致。
  - `account_not_configured`：未配置激活账号，无法执行相关操作（如 refresh）。
- **410 Gone**：候选登录会话已超过 15 分钟 TTL 并失效 (`candidate_expired`)。
- **503 Service Unavailable**：网关转发时未配置有效激活账号与 Key。

### 3.3 规范错误码枚举 (`code`)

| 错误码 (`code`) | 产生场景与说明 |
| :--- | :--- |
| `invalid_credentials` | 凭据错误。仅在上游明确返回凭据错误证明时使用。 |
| `upstream_challenge` | 上游返回了 WAF / 防火墙挑战页面，当前自动化流程无法自动解出。 |
| `upstream_session_expired`| 上游登录态失效，无法继续拉取数据。 |
| `upstream_session_unverified` | 上游登录后无法验证有效会话（如未能成功提取会话 Cookie）。 |
| `upstream_login_failed` | 上游登录接口提交失败或非预期错误。 |
| `login_form_unavailable` | 页面上未能找到或无法填入登录表单（如 DOM 变更或防护页面拦截）。 |
| `helper_input_invalid` | 发送给 helper 进程的参数格式错误或损坏。 |
| `browser_unavailable` | 未在 `.#browser` shell 下启动，缺少修补版 CloakBrowser 可执行文件。 |
| `upstream_timeout` | 上游连接、页面加载或数据接口请求超时。 |
| `upstream_unavailable` | 上游服务连接超时、502、503、504 或网络不可达。 |
| `upstream_unexpected_response` | 上游返回了非预期格式、畸形 JSON 或未兼容的 HTML。严禁将上游原始 HTML 注入 message。 |
| `key_material_unavailable` | 目标 Key 材料仅提供掩码或无法读取完整明文。系统**严禁自动创建新 Key，严禁猜测 `sk-` 前缀**。 |
| `persistence_failed` | 本地原子写入 `state` 文件（0600）失败。此时必须保持原有 active 状态不被破坏。 |
| `operation_in_progress` | 已存在处于 running 状态的异步 operation。 |
| `account_not_configured` | 当前没有已激活的 active 账号。 |

### 3.4 登录诊断选项 (`login_diagnostics`)
`config.toml` 提供可选配置项 `login_diagnostics = false`（默认关闭）：
- 开启时仅随 operation 错误返回固定白名单安全元数据（见 `backend/src/diagnostics.rs`），包含阶段耗时、安全错误码、HTTP 状态码及脱敏元数据，不额外落盘日志。
- 绝不返回或记录用户名、密码、原始 HTTP Headers、Set-Cookie、Cookie 内容或上游原始响应体。

---

## 4. 管理 API 接口契约 (10 Routes)

基础路径统一为 `/api`。

### 4.1 建立管理员会话
- **路径**：`POST /api/admin/session`
- **鉴权**：`Authorization: Bearer <root_key>`
- **请求体**：无
- **成功响应**：`204 No Content`
  - 响应头设置 `Set-Cookie: session_token=...; HttpOnly; SameSite=Strict; Path=/api/; Max-Age=28800`
- **失败响应**：`401 Unauthorized`（root_key 不匹配）

### 4.2 注销管理员会话
- **路径**：`DELETE /api/admin/session`
- **鉴权**：管理 Session Cookie
- **请求体**：无
- **成功响应**：`204 No Content`
  - 响应头设置 `Set-Cookie: session_token=; HttpOnly; SameSite=Strict; Path=/api/; Max-Age=0`

### 4.3 获取当前完整账户状态
- **路径**：`GET /api/account`
- **鉴权**：管理 Session Cookie
- **请求体**：无
- **成功响应**：`200 OK`，返回 `AccountState` JSON 对象。

### 4.4 登录并获取候选凭据 (异步任务)
- **路径**：`POST /api/account/login`
- **鉴权**：管理 Session Cookie
- **请求体**（示例仅为格式占位符，非真实凭据）：
  ```json
  {
    "username": "user@example.com",
    "password": "<account-password>"
  }
  ```
- **处理逻辑**：
  1. 校验当前是否已有 operation，若有则同步返回 `409 Conflict` (`operation_in_progress`)。
  2. 采用混合登录流程（Hybrid Login）：
     - a. 优先以独立的空标准 CookieStore 直接向固定地址 `POST /api/user/login` 发起 HTTP 请求。
     - b. 若上游直接返回有效 JSON，完成认证并直接复用验证得到的 self/id、balance 和 username，无需重复向上游发起 GET 请求。
     - c. 仅在明确收到已知 WAF 挑战 HTML 页面时，回退单次拉起 Browser Helper 尝试加载防护页并登录（持有单一浏览器全局信号量，直至子进程完全确认回收）。
     - d. 若直接 HTTP 请求返回 JSON 业务错误、超时、5xx 服务端错误或未知 HTML/3xx 响应，坚决不启动浏览器，直接报告对应错误。
     - e. 全局统一登录超时覆盖认证、self 校验与 token 列表获取全流程，流程取消或重试不重置截止时间。
  3. 获取 token 列表生成 `KeySummary[]`（共享网关 HTTP 客户端无 cookie jar）。
  4. 结果暂存为内存中的 `candidate` 对象（TTL 15 分钟），不直接替换 active。
  5. 密码仅在内存中供本次调用使用，随后立即丢弃，**严禁落盘，但未做底层安全内存擦除（not secure-wipe）**。
- **成功响应**：`202 Accepted`
  ```json
  {
    "operation_id": "op-login-uuid"
  }
  ```

### 4.5 激活指定候选 Key (异步任务)
- **路径**：`POST /api/account/activate`
- **鉴权**：管理 Session Cookie
- **请求体**：
  ```json
  {
    "candidate_id": "cand-uuid",
    "key_id": "upstream-token-id"
  }
  ```
- **处理逻辑**：
  1. 校验 candidate 存在且未过期（若过期返回 `410 Gone`）。
  2. 验证 `key_id` 属于该 candidate。
  3. 尝试读取该 Key 的完整已有明文材料。
     - 若上游仅提供掩码或无法读取，立即终止并报错 `key_material_unavailable`。
     - **严禁**自动调用上游接口创建新 Key，**严禁**自行猜测 `sk-` 等前缀。
  4. 将会话材料与选中 Key 原子写入本地 `state` 文件（`0600` 权限）。
  5. **只有持久化成功后**，才正式更新 `active`，清理 `candidate`，递增 `revision`。若持久化失败，保持旧 `active` 不变。
- **成功响应**：`202 Accepted`
  ```json
  {
    "operation_id": "op-activate-uuid"
  }
  ```

### 4.6 显式明文查看当前 Key
- **路径**：`POST /api/account/key/reveal`
- **鉴权**：**双重鉴权**（同时需要管理 Session Cookie 与 `Authorization: Bearer <root_key>` Header）
- **请求体**：
  ```json
  {
    "active_revision": "rev-1"
  }
  ```
- **处理逻辑**：
  1. 校验 root key 与 session 均有效。
  2. 校验 `active_revision` 是否匹配当前版本，不匹配返回 `409 Conflict` (`stale_revision`)。
  3. 从受限持久状态中读取当前 Key 原文返回。
- **成功响应**（示例仅为格式占位符，非真实凭据）：
  ```json
  {
    "active_revision": "rev-1",
    "key_id": "upstream-token-id",
    "key": "<upstream-api-key>"
  }
  ```
- **前端规范**：仅在前端组件状态中短时展示，提供显式的“隐藏/清除”操作，禁止持久化存储。

### 4.7 刷新激活账户余额 (异步任务)
- **路径**：`POST /api/account/refresh`
- **鉴权**：管理 Session Cookie
- **请求体**：无
- **处理逻辑**：
  1. 检查是否存在 `active` 账号。若不存在，同步返回 `409 Conflict` (`account_not_configured`)。
  2. 异步触发 `/api/user/self` 请求更新余额。
  3. 刷新失败时保留现有的余额与 Key，不破坏现有会话，严禁隐式切换或清除账号。
- **成功响应**：`202 Accepted`
  ```json
  {
    "operation_id": "op-refresh-uuid"
  }
  ```

### 4.8 获取签到状态与历史
- **路径**：`GET /api/checkin`
- **鉴权**：管理 Session Cookie 鉴权，强制校验 `Origin` / `Host`，响应头包含 `Cache-Control: no-store`
- **请求体**：无
- **成功响应**：`200 OK`，返回 `CheckinFullStatus` JSON 对象（包含调度配置、服务端计算的 `cycle_date`、`today` 记录、按当前账号过滤的历史摘要 `history` 及 `next_run_at`；详见 `docs/checkin-contract.md`）。

### 4.9 更新定时签到配置
- **路径**：`PUT /api/checkin/settings`
- **鉴权**：管理 Session Cookie 鉴权，强制校验 `Origin` / `Host`，响应头包含 `Cache-Control: no-store`
- **请求体**：
  ```json
  {
    "enabled": true,
    "time": "09:30"
  }
  ```
- **处理规范**：`time` 必须严格满足 24 小时制 `HH:MM`；成功返回 `200 OK` 及更新后的完整 `CheckinFullStatus`。

### 4.10 手动触发签到 (异步任务)
- **路径**：`POST /api/checkin/run`
- **鉴权**：管理 Session Cookie 鉴权，强制校验 `Origin` / `Host`，响应头包含 `Cache-Control: no-store`
- **请求体**：
  ```json
  {
    "confirm_retry": false
  }
  ```
- **处理规范**：若当前账号在当前周期已存在 `success` 或 `already_done`，直接返回 `200 OK` (`{"already_recorded": true}`)；未确认且未传 `confirm_retry: true` 时返回 `409` (`checkin_retry_confirmation_required`)；校验通过返回 `202 Accepted` (`{"operation_id": "..."}`)，先原子落盘 `running` 意图再发起轻量 HTTP 请求。

---

## 5. Gateway 代理规范 (`/v1` 与 `/v1/*`)

AnyRouter Manager 提供透明反向代理网关，供 OpenAI 兼容客户端调用。

### 5.1 鉴权与路由匹配
- **匹配规则**：拦截所有 HTTP 方法的 `/v1` 及 `/v1/*` 请求。
- **客户端鉴权**：客户端必须携带 `Authorization: Bearer <root_key>`。若不匹配，网关直接返回 `401 Unauthorized`。

### 5.2 转发与报文处理规范
1. **目标地址**：固定转发至 `https://anyrouter.top`，保持 1:1 的原始 HTTP Method、原始请求路径 (Raw Path) 与 Query 字符串。
2. **凭据替换**：
   - 提取当前 `active.selected_key` 的明文 API Key。
   - 将发往上游请求中的 `Authorization` 请求头替换为 `Bearer <selected_key>`。
3. **敏感头剥离（防泄露）**：
   - 严禁向上游透传本地 `root_key`。
   - 严禁透传本地管理 Cookie (`session_token`) 及任何管理端专用请求头。
4. **Hop-by-hop Headers 剥离**：
   - 必须剥离标准逐跳标头：`Connection`、`Keep-Alive`、`Proxy-Authenticate`、`Proxy-Authorization`、`TE`、`Trailers`、`Transfer-Encoding`、`Upgrade`。
   - 同时解析并剥离 `Connection` 标头中列出的所有自定义逐跳标头。
5. **Host 头处理**：由客户端代理层设置为目标上游主机名：`Host: anyrouter.top`。
6. **重定向控制**：网关**不得自动跟随重定向**（`redirect: manual` / nofollow）。若上游返回 3xx，直接将该状态码与 `Location` 标头原样返回客户端。
7. **数据流传输 (Streaming & Bytes)**：
   - **禁止对 Request Body 与 Response Body 做 JSON 反序列化与重新编码**。
   - 必须以 Raw Bytes 形式进行全双工流式传输（Streaming），完整透传 SSE (Server-Sent Events) 数据块。
8. **缺失凭据处理**：若后端当前未配置激活的 `active` 账号或无有效 Key，网关直接返回 `503 Service Unavailable`，响应安全错误提示，不向上游发起任何请求。
9. **网络与安全部署说明**：
   - 生产环境进程默认只监听 `127.0.0.1:8080`。
   - 若需支持远程或公网访问，部署者必须在外层配置安全的 TLS 反向代理（如 Nginx 或 Caddy），严禁在无 TLS 保护的公网环境中明文暴露。
