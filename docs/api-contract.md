# AnyRouter Manager API 契约与技术规范

本文档定义 AnyRouter Manager 后端与前端之间的接口契约、数据模型、安全规范及网关行为。供后端（Backend Fixer）与前端（Designer / Frontend Fixer）共同遵守。

---

## 1. 架构与技术基准

| 配置项 | 规范要求 |
| :--- | :--- |
| **系统架构** | 多账号模式（Portfolio V2，最多支持 64 个账号）；Rust 后端（提供管理 API 与 `/v1` 网关）+ React Vite 前端（TypeScript）。全局同时仅由管理员显式选定一个账号作为生效代理路由（`route_account_id`），不提供黑盒负载均衡、隐式轮询或自动故障切换；多账号固定时槽签到功能已接入前后端接口（详见 `docs/checkin-contract.md`）。前端多账号面板当前处于独立组件对接待完成阶段。 |
| **固定上游** | 固定为 `https://anyrouter.top`。禁止通过请求头、查询参数或路径覆盖上游地址（防 SSRF）。 |
| **系统凭证** | 由本地 `config.toml` 配置 `root_key`（至少 32 字节高熵随机 ASCII），统一作为本地管理鉴权 (`/api/admin/*`)、`/v1/*` 客户端代理请求以及 `reveal` 查看 Key 明文的二次鉴权凭证。切勿将密钥提交至公开代码仓库。 |
| **状态持久化与安全边界** | 本地状态文件采用私有权限 `0600`（父目录 `0700`）与原子写，包含账号状态 `data/account.json`（Portfolio V2 格式，上限 8 MiB）、签到状态 `data/checkin.json`（上限 8 MiB）与请求日志 `data/request-logs.json`（上限 1 MiB）。<br>• **注意：这是敏感明文存储，不是加密保险箱（0600 plaintext, not encrypted vault）**，不存储用户登录密码。<br>• 用户名与密码仅在登录时驻留短期内存供调用，**严禁落盘，但未做底层安全内存擦除（not secure-wipe）**。<br>• 系统无法承诺必然绕过未来上游变更后的 WAF、ESA 或人机挑战。 |
| **服务端口与部署** | 后端默认监听 `127.0.0.1:8080`。<br>• **运行时模式配置**：支持 `container_mode = false`（默认 false，显式设为 true 时方可监听非回环地址并启用 supervisor 监督模式）；`cookie_secure = false`（默认 false，要求远程 HTTPS 反向代理部署时启用以打上 Secure 标记）。<br>• **开发环境**：Vite 监听 `127.0.0.1:5173`，已配置 dev proxy 将 `/api` 与 `/v1` 同源代理至 `127.0.0.1:8080`。<br>• **生产环境**：Rust 后端直接托管 `frontend/dist` 静态资源；对于未匹配的静态资源请求，返回 `404` 状态码及 SPA HTML 页面体。 |
| **管理会话机制** | 使用 `HttpOnly; SameSite=Strict; Path=/api/` Cookie，有效期 8 小时。前端**禁止**将会话凭据放入 `localStorage`、`sessionStorage` 等客户端持久存储。 |
| **安全请求校验** | 后端严格校验所有管理写请求的 `Origin` 与 `Host` 头，防止跨站或劫持请求。 |
| **缓存策略** | 所有 `/api/*` 管理接口响应头均强制下发 `Cache-Control: no-store`，禁止任何中间代理与浏览器缓存。 |
| **Cookie 隔离与过期行为** | 来自上游 AnyRouter 的所有 Cookie 仅保存在后端受控状态中，严禁下发给前端或客户端。<br>• **过期加载行为**：若上游 Session Cookie 过期，已激活的 API Key 仍可继续支撑 `/v1` 网关代理请求；仅在触发 `refresh` 时向管理端报错 `upstream_session_expired`。 |
| **数据格式标准** | • 所有实体 ID 必须为字符串 (`string`)。<br>• 时间戳统一使用 ISO 8601 UTC 格式（支持以 `Z` 或 `+00:00` 表示，如 `2026-10-01T12:00:00Z`）。<br>• 额度字段 `quota_raw` 与 `used_quota_raw` 为十进制字符串（如 `"1000000"`），禁止后端或协议层做美元换算。 |
| **优雅停机与任务管理** | 后端原生捕获 `SIGTERM` 与 `SIGINT` 信号；收到信号后停止接收新任务，在 10 秒内清理后台跟踪任务，15 秒内排空存量 HTTP 请求后优雅退出。 |
| **Browser Helper 与混合登录** | 采用混合登录架构（HTTP-First）：优先直接发起 HTTP POST 登录；仅在明确收到已知 WAF 挑战 HTML 时单次回退启动 Browser Helper（遇 5xx/超时/未知 HTML 不回退）。受全局单一浏览器信号量保护（从 spawn 直至进程完全确认回收，含中断与取消）。本地非容器环境通过 Linux namespace 与 `PDEATHSIG` 调用 helper 隔离进程；`container_mode = true` 下采用 Rust subreaper 监督进程（supervisor）模式管理 helper 进程树，配合容器 PID namespace 与 `tini`，无需 `SYS_ADMIN` 或 `unconfined`，禁止 `host PID`。可选配置 `browser_helper_executable` 指定 Python 解释器绝对路径。若在基础 shell 或缺少修补浏览器环境下启动，登录接口返回 `browser_unavailable`。 |

---

## 2. 数据传输对象契约 (DTO Specifications)

### 2.1 多账号状态对象 `AccountsStatusDTO`

管理端核心全景接口 `GET /api/accounts` 返回的对象：

```typescript
export interface AccountsStatusDTO {
  accounts: AccountDTO[];
  route_account_id: string | null;
  candidate: CandidateDTO | null;
  operation: OperationDTO | null;
}
```

#### 子对象定义

```typescript
export interface KeySummaryDTO {
  id: string; // 令牌数字 ID
  name: string; // 令牌名称
  masked: string; // 脱敏展示（沿用既有 mask 实现）
  enabled: boolean; // 是否启用
}

export interface AccountDTO {
  id: string; // 本地分配的 UUID
  revision: string; // 乐观锁版本号（opaque string）
  username: string; // 上游用户名 / 邮箱
  upstream_user_id: string; // 上游用户 ID
  balance: AccountBalanceDTO;
  keys: KeySummaryDTO[]; // 令牌元数据列表
  selected_key: KeySummaryDTO | null; // 当前选中的代理令牌（脱敏格式）
  added_at: string; // ISO 8601 UTC
}

export interface CandidateDTO {
  id: string; // 临时 UUID
  username: string;
  upstream_user_id: string;
  balance: AccountBalanceDTO;
  keys: KeySummaryDTO[];
  expires_at: string; // ISO 8601 UTC，内存暂存 15 分钟
}

export interface AccountBalanceDTO {
  quota_raw: string; // 原始额度数值（十进制字符串）
  used_quota_raw: string; // 已用额度数值（十进制字符串）
  fetched_at: string; // ISO 8601 UTC
}

export interface OperationDTO {
  id: string; // operation_id
  account_id: string | null;
  kind: "login" | "save" | "refresh" | "select_key" | "checkin";
  status: "running" | "succeeded" | "failed";
  phase: string;
  error: OperationError | null;
}

export interface OperationError {
  code: string;
  message: string;
  source?: "helper" | "credentials" | "self" | "tokens" | "key" | "persistence";
}

export interface RequestLogDTO {
  id: string;
  timestamp: string; // ISO 8601 UTC
  account_id: string | null;
  account_name: string | null;
  http_status: number | null; // null 表示未收到上游响应
  error_body: string | null; // 仅非 2xx 保留原文前 64 KiB，2xx 恒为 null
  truncated: boolean; // 是否截断
}
```

---

## 3. 管理 API 路由契约

基础路径为 `/api`。全部接口强制 Session Cookie 鉴权、响应强制下发 `Cache-Control: no-store`。所有写操作（`POST`/`PUT`/`DELETE`）强制校验请求的 `Origin` 与 `Host` 请求头。

### 3.1 获取账号全景状态
- **路径**：`GET /api/accounts`
- **鉴权**：管理 Session Cookie
- **响应**：`200 OK`，返回 `AccountsStatusDTO`

### 3.2 发起账号登录尝试 (异步)
- **路径**：`POST /api/accounts/login`
- **请求体**：
  ```json
  {
    "username": "user@example.com",
    "password": "<account-password>"
  }
  ```
- **行为**：校验无并发冲突；启动 HTTP-First 混合登录（遇已知 WAF 回退单次浏览器）；成功后在内存暂存为 `Candidate` 对象（TTL 15 分钟）。密码严禁落盘。
- **响应**：`202 Accepted`，返回 `{"operation_id": "..."}`

### 3.3 保存候选账号至列表 (异步)
- **路径**：`POST /api/accounts/save`
- **请求体**：
  ```json
  {
    "candidate_id": "b1a2c3d4-...",
    "key_id": "12345"
  }
  ```
- **行为**：校验账号总数不超过 64；对已有 `upstream_user_id` 执行合并，新账号追加至末尾；提取并保存明文 Key 至 `0600` 状态文件。**保存新账号绝不自动切换为全局生效路由**。
- **响应**：`202 Accepted`，返回 `{"operation_id": "..."}`

### 3.4 刷新指定账号信息 (异步)
- **路径**：`POST /api/accounts/{id}/refresh`
- **请求体**：空 JSON `{}`
- **行为**：使用该账号持久化 Cookie 请求 `/api/user/self` 刷新余额与令牌列表；Cookie 过期报 `upstream_session_expired`，不清除现有 API Key。
- **响应**：`202 Accepted`，返回 `{"operation_id": "..."}`

### 3.5 变更账号绑定的代理 Key (异步)
- **路径**：`POST /api/accounts/{id}/key/select`
- **请求体**：
  ```json
  {
    "revision": "3",
    "key_id": "67890"
  }
  ```
- **行为**：乐观锁校验 `revision`；更新该账号的 `selected_key`，**不切换网关的生效路由账号**。
- **响应**：`202 Accepted`，返回 `{"operation_id": "..."}`

### 3.6 查看选定 Key 明文 (二次鉴权，同步)
- **路径**：`POST /api/accounts/{id}/key/reveal`
- **鉴权**：管理 Session Cookie + `Authorization: Bearer <root_key>` 双重鉴权（不使用自定义标头或请求体传递 root_key）。
- **请求体**：
  ```json
  {
    "revision": "3"
  }
  ```
- **响应**：`200 OK`
  ```json
  {
    "account_id": "a1b2c3d4-...",
    "revision": "3",
    "key_id": "67890",
    "key": "<upstream-api-key>"
  }
  ```

### 3.7 显式切换全局生效路由 (同步)
- **路径**：`POST /api/accounts/{id}/route`
- **请求体**：
  ```json
  {
    "revision": "3"
  }
  ```
- **行为**：校验目标账号拥有有效选定的 API Key；若无 Key 返回 `422 Unprocessable Entity` (`account_has_no_valid_key`)；乐观锁校验通过后原子更新 `route_account_id` 并递增版本。后续新请求立即生效，存量请求保持旧快照。
- **响应**：`200 OK`，返回最新的 `AccountsStatusDTO`。

### 3.8 查询请求元数据与错误日志
- **路径**：`GET /api/request-logs?limit=100`
- **鉴权**：管理 Session Cookie
- **参数**：`limit`（可选，默认 100，最大 1000）
- **响应**：`200 OK`
  ```json
  {
    "items": [
      {
        "id": "log-uuid-1",
        "timestamp": "2026-10-01T10:00:00.123Z",
        "account_id": "a1b2c3d4-...",
        "account_name": "user@example.com",
        "http_status": 429,
        "error_body": "{\"error\":{\"message\":\"Rate limit exceeded\"}}",
        "truncated": false
      }
    ],
    "dropped_count": 0
  }
  ```

### 3.9 历史兼容与废弃端点
- **`GET /api/account`**：只读兼容端点。若存在生效路由，投影返回 `{ "active": ... }`；若未配置路由，返回 `{ "active": null }`。
- **旧版写入端点**：旧版 `POST /api/account/login`、`POST /api/account/activate` 等直接修改单账号的端点，全部统一返回 **`410 Gone`**（错误码 `deprecated_endpoint`）。

---

## 4. 网关行为与模型过滤规范 (`/v1/*`)

1. **路由绑定与快照隔离**：
   - 外部客户端携带 `Authorization: Bearer <root_key>` 请求网关接口。
   - 准入时获取当前生效路由账号的不可变快照（`immutable route snapshot`），将请求头替换为该账号的实际 API Key 转发至上游。
   - 正在执行的流式响应（SSE）全程绑定发起时的快照，管理员中途切换路由对在途连接无影响。若未配置任何生效路由账号，返回 `503 Service Unavailable`。
2. **`/v1/models` 精确剔除 13 个模型 ID**：
   - 当上游返回 200 OK、体积 $\le 2\text{ MiB}$ 且为有效 JSON 对象（包含 `data` 数组）时，严格剔除以下 13 个模型 ID：
     `gpt-5-codex`、`claude-3-5-haiku-20241022`、`claude-3-5-sonnet-20241022`、`claude-3-7-sonnet-20250219`、`claude-haiku-4-5-20251001`、`claude-opus-4-1-20250805`、`claude-opus-4-20250514`、`claude-opus-4-5-20251101`、`claude-opus-4-6`、`claude-opus-4-7`、`claude-sonnet-4-20250514`、`claude-sonnet-4-5-20250929`、`gemini-2.5-pro`。
   - 保留其余模型顺序与属性；剥离客户端条件标头并强设 `Accept-Encoding: identity`；修改响应后剥离失效标头并重算 `Content-Length`。
   - 不拦截或篡改客户端推理请求中的任何模型参数。
3. **其它接口透明穿透**：
   - `/v1/chat/completions` 等推理接口保持 1:1 双向透明穿透，不缓存完整 SSE 流。
