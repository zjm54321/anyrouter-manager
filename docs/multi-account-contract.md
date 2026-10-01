# AnyRouter Manager 多账号与网关增强技术契约 (Portfolio V2)

本文档定义 AnyRouter Manager 多账号管理体系（Portfolio V2）、路由切换机制、多账号固定时槽签到系统、请求元数据与错误日志收集、`/v1/models` 模型精确过滤以及容器持续集成规范。供后端（Backend Fixer）、前端（Frontend Fixer）、模块实现者及集成验证者作为冻结接口契约遵守。

> **基线与状态声明**：
> 1. 本契约建立在单账号基线（前 10 次提交）全部通过的成果之上，保持既有单账号安全与生命周期边界。
> 2. 本文档为系统设计与接口契约规范，当前各模块实现处于并行开发中（实现中），前端多账号面板当前处于独立组件对接待完成阶段，用户部署上线待本地手动确认执行，文档内不预称新增测试已通过。
> 3. 系统严格禁止在普通 DTO 或前端常规展示中泄漏用户真实登录密码与完整 API Key（完整 Key 仅保存在 0600 本地状态凭据中，并在显式 reveal 操作经管理 Session 与 `Authorization: Bearer <root_key>` 双重鉴权后返回；系统不额外采集客户端请求体、auth 标头或密码；非 2xx 请求日志依据用户选择保留上游原始错误正文，若上游自身回显 Key 则按原文保留）。

---

## 1. 核心设计原则

1. **单路由显式受控代理**：
   - 系统支持管理最多 64 个 AnyRouter 账号，但在任意时刻，`/v1` 网关代理有且仅有一个明确选定的生效路由账号（`route_account_id`）。
   - 绝不引入客户端请求级别的黑盒负载均衡、隐式轮询（round-robin）或故障自动切换（fallback）。
   - 切换路由必须由管理员通过接口显式触发，且目标账号必须已成功关联有效的 API Key。
2. **请求快照与流式传输隔离**：
   - 每一个到达 `/v1` 的外部推理请求，在通过准入鉴权时捕获当前生效账号的不可变快照（`immutable route snapshot`，包含凭据与 Key）。
   - 运行中（in-flight）的流式请求（SSE）与长连接全程绑定其启动时的快照，管理员在界面切换路由完全不影响存量连接，严禁中途换号续流。
3. **安全存储与多重隔离**：
   - 运行时完全不依赖外部数据库，所有状态通过原子写落盘至专用 JSON 状态文件。
   - 文件权限严格限定为 `0600`（所有父级目录严格为 `0700`）。账号状态与签到状态各设 8 MiB 大小上限；请求日志设 1 MiB 大小上限（沿用既有实现上限示范值）。
   - 所有状态文件路径在初始化时进行规范化检查，避免实际别名冲突（如软链接、相对路径或 `..` 跃迁指向同一物理文件）；多个状态文件存放在同一合法私有目录下属于正常支持范围，系统确保路径互不重叠冲突且不覆盖 `config.toml`。
4. **受管生命周期与单浏览器互斥**：
   - 全局维持单一浏览器信号量（Concurrency = 1）。HTTP-First 机制优先；仅在遇到已知 WAF 挑战时单次按需拉起 Browser Helper，无常驻 Chromium。
   - 单进程串行调度等待期间严禁持有任何全局锁或浏览器资源。

---

## 2. 存储模型与迁移规范 (Portfolio V2)

### 2.1 存储结构定义

本地账号状态文件（默认 `data/account.json`）升级为版本 2 格式：

```typescript
export interface PortfolioV2 {
  version: 2;
  accounts: AccountEntry[];
  route_account_id: string | null;
}

export interface AccountEntry {
  id: string; // 本地生成的独立 UUIDv4 字符串，全局唯一
  revision: string; // 乐观锁版本号（opaque string，如历史 uuid 或递增标识）
  username: string; // 上游用户名 / 邮箱
  upstream_user_id: string; // 上游用户数字 ID（如 "12345"），全系统唯一排重键
  balance: {
    quota_raw: string; // 上游额度原始十进制字符串（raw unit）
    used_quota_raw: string; // 上游已用额度原始十进制字符串
    fetched_at: string; // ISO 8601 UTC 时间戳（支持 Z 或 +00:00）
  };
  credentials: {
    cookies: CookieItem[]; // 上游会话 Cookie 集合（沿用既有合法 Credentials 约束）
    api_user: string; // 必须与 upstream_user_id 完全一致
  };
  keys: KeySummaryItem[]; // 随登录或 refresh 抓取的令牌元数据列表（沿用既有上限约束）
  selected_key: SelectedKeyItem | null; // 当前账号选定的用于代理的完整 Key
  added_at: string; // 账号初次加入系统的 ISO 8601 UTC 时间戳（支持 Z 或 +00:00）
}

export interface SelectedKeyItem {
  id: string; // 令牌数字 ID
  name: string; // 令牌名称
  key: string; // 完整明文 API Key（仅存保密状态文件，严禁入普通日志与常规 DTO）
}
```

### 2.2 存储容量与约束规则
1. **账号上限与唯一性**：
   - 系统支持的最大账号数为 **64**（存储上限 8 MiB 沿用既有实现上限）。超过 64 时拒绝添加新账号（返回 `409 Conflict`，错误码 `account_limit_reached`）。
   - 以 `upstream_user_id` 作为物理唯一性约束。若添加的账号其 `upstream_user_id` 已存在：
     - 若该账号已处于 `accounts` 中，走重新认证更新流程（保留其原始 `id`、列表顺序 `order` 和既有 `route_account_id` 指向，更新会话与 Key 列表）。
2. **有序性保证**：
   - `accounts` 列表必须严格保持添加顺序（insertion order），签到调度时序严格按照此顺序分配固定时槽。
3. **Key 的可选性**：
   - 新增账号时允许 `selected_key: null`。无选定 Key 的账号完全支持参与管理、刷新余额和每日签到，但**不可被选为路由代理账号**。
4. **Legacy Active 迁移机制**：
   - 当检测到状态文件不存在 `version` 字段时，判定为单账号旧版 `Active` 格式。
   - 迁移器读取旧版 `Active` 数据，校验其结构有效性；生成新的本地 UUID 并构造 `AccountEntry`，保留其完整凭据、余额、选中 Key 及 revision。
   - 初始化 `route_account_id` 指向该迁移账号。
   - 执行原子落盘前校验，若写入失败或校验不通过，**必须完整保留原旧文件原始字节不变**，服务安全退出。
5. **路径安全防别名机制**：
   - 账号状态路径（`state_path`）、签到状态路径（`checkin_state_path`）、日志路径（`request_log_path`）以及配置文件路径（`config.toml`），在解析真实路径后必须互不指向同一物理文件，避免别名覆盖。
   - 多个状态文件存放在同一私有目录下属于正常配置，系统仅校验实际路径无别名冲突且不覆盖 `config.toml`。

---

## 3. 数据传输对象契约 (DTO Schemas)

所有时间戳统一使用 ISO 8601 UTC 格式（支持以 `Z` 或 `+00:00` 表示，兼容 chrono RFC3339 与既有 `now()` 实现）。所有实体 ID 统一使用字符串（新增账号分配本地 UUID 字符串）。

### 3.1 令牌与账号 DTO
```typescript
export interface KeySummaryDTO {
  id: string; // 令牌数字 ID（字符串形式）
  name: string; // 令牌名称
  masked: string; // 脱敏遮罩值（沿用既有 mask 实现）
  enabled: boolean; // 是否启用（新增契约字段）
}

export interface AccountDTO {
  id: string; // 本地分配的 UUID
  revision: string; // 当前版本号（opaque string）
  username: string; // 账号名称
  upstream_user_id: string; // 上游用户 ID
  balance: {
    quota_raw: string; // 原始额度数值
    used_quota_raw: string; // 原始已用额度数值
    fetched_at: string; // 抓取时间
  };
  keys: KeySummaryDTO[]; // 令牌元数据列表（免除单独 GET /api/keys）
  selected_key: KeySummaryDTO | null; // 当前选中的令牌（仍为脱敏格式）
  added_at: string; // 添加时间
}
```

### 3.2 登录候选凭证 (Candidate DTO)
```typescript
export interface CandidateDTO {
  id: string; // 候选对象临时 UUID
  username: string;
  upstream_user_id: string;
  balance: {
    quota_raw: string;
    used_quota_raw: string;
    fetched_at: string;
  };
  keys: KeySummaryDTO[];
  expires_at: string; // 候选凭据有效期（内存 TTL 15 分钟）
}
```

### 3.3 异步任务 (Operation DTO)
```typescript
export interface OperationDTO {
  id: string; // operation_id (UUID)
  account_id: string | null; // 关联的账号 ID（如适用）
  kind: "login" | "save" | "refresh" | "select_key" | "checkin";
  status: "running" | "succeeded" | "failed";
  phase: string;
  error: {
    code: string;
    message: string;
    source?: "helper" | "credentials" | "self" | "tokens" | "key" | "persistence";
  } | null;
}
```

### 3.4 综合状态对象 (AccountsStatus DTO)
`GET /api/accounts` 统一返回全景状态：
```typescript
export interface AccountsStatusDTO {
  accounts: AccountDTO[];
  route_account_id: string | null;
  candidate: CandidateDTO | null;
  operation: OperationDTO | null;
}
```

---

## 4. 管理 API 路由契约

基础路径为 `/api`。全部接口强制 Session Cookie 鉴权、响应强制下发 `Cache-Control: no-store`。所有写操作（`POST`/`PUT`/`DELETE`）强制校验请求的 `Origin` 与 `Host` 请求头。

### 4.1 获取账号全景状态
- **路径**：`GET /api/accounts`
- **鉴权**：管理 Session Cookie
- **响应**：`200 OK`，返回 `AccountsStatusDTO`

### 4.2 发起账号登录尝试 (异步)
- **路径**：`POST /api/accounts/login`
- **请求体**：
  ```json
  {
    "username": "user@example.com",
    "password": "<account-password>"
  }
  ```
- **行为规范**：
  1. 若当前存在正在执行的后台操作，返回 `409 Conflict` (`operation_in_progress`)。
  2. 采用 HTTP-First 混合登录流程（直连 POST 失败遇已知 WAF 单次回退浏览器）。
  3. 认证成功后拉取余额与令牌列表，在内存中暂存为 `Candidate` 对象（TTL 15 分钟）。
  4. 密码仅在内存中供本次调用使用，随后立即丢弃，**严禁落盘**。
- **响应**：`202 Accepted`，返回 `{"operation_id": "..."}`

### 4.3 保存候选账号至列表 (异步)
- **路径**：`POST /api/accounts/save`
- **请求体**：
  ```json
  {
    "candidate_id": "b1a2c3d4-...",
    "key_id": "12345" // 可选，传 null 或省略表示暂不绑定 Key
  }
  ```
- **行为规范**：
  1. 校验 `candidate_id` 有效且未过期，校验账号总数不超过 64。
  2. 若该 `upstream_user_id` 为已有账号，执行属性合并；若为新账号，追加到列表末尾。
  3. 若提供了 `key_id`，提取对应的完整明文 Key 写入存储中的 `selected_key`。
  4. **注意**：新增或保存账号**绝对不自动将其切换为当前代理路由**（保持原有 `route_account_id` 不变）。
- **响应**：`202 Accepted`，返回 `{"operation_id": "..."}`

### 4.4 刷新指定账号信息 (异步)
- **路径**：`POST /api/accounts/{id}/refresh`
- **请求体**：空 JSON `{}`
- **行为规范**：
  1. 使用目标账号保存在后端的会话 Cookie 请求上游 `/api/user/self` 校验身份并刷新余额。
  2. 重新拉取令牌列表更新 `keys` 元数据。若原选中的 Key 仍在上游列表中，保留该 `selected_key`。
  3. 若上游 Cookie 已过期，报告 `upstream_session_expired`，不清除现有 API Key。
- **响应**：`202 Accepted`，返回 `{"operation_id": "..."}`

### 4.5 变更账号绑定的代理 Key (异步)
- **路径**：`POST /api/accounts/{id}/key/select`
- **请求体**：
  ```json
  {
    "revision": "3",
    "key_id": "67890"
  }
  ```
- **行为规范**：
  1. 校验 `revision` 匹配（乐观锁，不匹配返回 `409 Conflict`）。
  2. 校验 `key_id` 存在于账号已抓取的令牌列表内，提取对应完整 Key 原子持久化。
  3. 此接口仅更新该账号的 `selected_key`，**不切换网关的全局代理路由**。
- **响应**：`202 Accepted`，返回 `{"operation_id": "..."}`

### 4.6 查看选定 Key 明文 (二次鉴权，同步)
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

### 4.7 显式切换全局代理路由 (同步)
- **路径**：`POST /api/accounts/{id}/route`
- **请求体**：
  ```json
  {
    "revision": "3"
  }
  ```
- **行为规范**：
  1. 校验目标账号存在且其 `selected_key` 非空且结构有效。若账号未配置有效 Key，拒绝切换并返回 `422 Unprocessable Entity` (`account_has_no_valid_key`)。
  2. 校验 `revision` 符合乐观锁。
  3. 原子更新 `route_account_id` 指向该账号，并递增版本号。
  4. 随后的全新 `/v1` 客户端请求将立即采用该账号作为代理凭据；在途（in-flight）长连接保持原有快照不变。
- **响应**：`200 OK`，返回最新的完整 `AccountsStatusDTO`。

### 4.8 历史兼容与废弃端点
- **`GET /api/account`**：保留兼容只读端点。若当前存在有效 `route_account_id`，将其投影为单账号旧版 `Active` 格式 DTO；若未选定路由，返回 `{"active": null}`。
- **废弃的写入端点**：旧版 `POST /api/account/login`、`POST /api/account/activate` 等直接修改单账号的端点，全部统一返回 **`410 Gone`**（错误码 `deprecated_endpoint`）。严禁利用旧版接口隐式选择或覆盖多账号路由。

---

## 5. 多账号固定时槽签到系统

### 5.1 签到周期与时间槽位算法

1. **统一周期基准**：
   - 沿用北京时间（CST，UTC+8）每日 08:00 为新周期起点的既有规则：
     $$\text{cycle\_date} = \text{date}(\text{now}_{\text{CST}} - 8\text{h}) \equiv \text{date}(\text{now}_{\text{UTC}})$$
   - 周期区间：$[D\text{ 08:00 CST}, D+1\text{ 08:00 CST})$。
2. **多账号固定时槽排程 (Fixed Time Slot Scheduling)**：
   - 单进程串行调度体系（非分布式系统）。所有在册账号按照添加至系统的严格顺序（索引 $i = 0, 1, \dots, N-1$）分配执行槽位。
   - 配置项包含每日基准起始时间 `time`（`HH:MM`）与账号间执行间隔 `interval_minutes`（整数，范围 $1 \le \text{interval\_minutes} \le 1440$）。
   - **槽位偏移量计算规则**（相对于周期起点 08:00 的分钟偏移 $\Delta t$）：
     - 设用户配置时间对应的自然日分钟数为 $M = \text{hour} \times 60 + \text{minute}$。
     - 若 $M \ge 480$（即处于当天 08:00 至 23:59 之间）：基准偏移 $\text{base\_offset} = M - 480$。
     - 若 $M < 480$（即处于次日 00:00 至 07:59 之间）：基准偏移 $\text{base\_offset} = M + 960$。
     - 账号 $i$ 在周期内的执行偏移量为：
       $$\text{offset}_i = \text{base\_offset} + i \times \text{interval\_minutes}$$
3. **排程越界保护 (`schedule_overflow`)**：
   - 签到周期总跨度固定为 24 小时（1440 分钟）。
   - 最后一个账号的执行偏移量必须严格落在当前周期内，即：
     $$\text{offset}_{N-1} < 1440$$
   - 在更新签到配置（`PUT /api/checkin/settings`）或添加新账号（`POST /api/accounts/save`）时，系统在同一互斥锁下计算预排程。若 $\text{offset}_{N-1} \ge 1440$，操作必须被拒绝并返回 **`422 Unprocessable Entity`**（错误码 `schedule_overflow`），阻止配置超限生效。

### 5.2 状态机与调度执行原则

1. **单账号单周期唯一性**：
   - 排重二元组为 `(account_user_id, cycle_date)`。
   - 无论自动排程还是手动触发，一旦在当前周期获得 `success` 或 `already_done`，该账号在本周期内彻底锁定，后续排程自动跳过。
2. **失败与未知状态克制性**：
   - 若某账号在排程中发生网络超时、WAF 拦截或报错，标记为 `failed` 或 `unknown`。
   - **本周期内绝不自动进行二次重试**。某个账号的执行耗时或失败完全不后移后续账号的预定槽位时间。
3. **忙碌排队与补偿运行 (Catch-Up)**：
   - 若到达某账号槽位时后台正在执行其他耗时操作（全局 busy），调度器在此操作完成后立即补跑（catch-up）当前周期内已过槽位但尚未执行的账号。
   - 调度器**绝不向前追溯补跑历史已终结周期的签到**。
   - 新加入系统的账号若其计算槽位已处于当前周期过去时间，在该周期内尚未尝试过的前提下，获得一次当即补偿运行机会。
4. **资源占用边界**：
   - 槽位间长达数十分钟的等待期间，调度器仅处于异步定时器休眠状态，**严禁持有任何状态互斥锁，绝不拉起或常驻浏览器**。
5. **历史存储限制与安全裁剪**：
   - 单账号保留最近最多 180 条历史记录；全系统历史记录总上限为 11,520 条；签到状态文件 `data/checkin.json` 上限 8 MiB。
   - **历史身份数与当前账号数区别**：系统支持最多 64 个活跃账号，但历史记录中出现过的不同身份总数不受 64 限制（旧的第 65+ 个历史身份记录完全可被正常加载与保留，不与当前活跃账号数混淆）。
   - **记录保护与安全裁剪**：清理历史记录时，**绝对保留当前周期、上一周期、未来周期及处于 running 状态的所有记录**（用于关键排重与跨日切对齐），历史满额时仅修剪更早的历史周期；过期的 `CycleAmbiguous` 记录在符合条件时可安全裁剪。

### 5.3 签到 API 契约

#### 全局签到状态与排程
- **路径**：`GET /api/checkin`
- **响应**：`200 OK`
  ```json
  {
    "settings": {
      "enabled": false,
      "time": "09:00",
      "interval_minutes": 30,
      "timezone": "Asia/Shanghai",
      "reset_time": "08:00"
    },
    "cycle_date": "2026-10-01",
    "next_run_at": "2026-10-02T01:00:00Z",
    "schedule": [
      {
        "account_id": "uuid-1",
        "scheduled_at": "09:00",
        "status": "success"
      },
      {
        "account_id": "uuid-2",
        "scheduled_at": "09:30",
        "status": "pending"
      }
    ]
  }
  ```

#### 更新全局签到配置
- **路径**：`PUT /api/checkin/settings`
- **请求体**：
  ```json
  {
    "enabled": true,
    "time": "09:00",
    "interval_minutes": 30
  }
  ```
- **响应**：成功返回 `200 OK` 及更新后的完整全局签到状态对象；若造成槽位溢出返回 `422` (`schedule_overflow`)。

#### 获取指定账号签到历史
- **路径**：`GET /api/accounts/{id}/checkin`
- **响应**：`200 OK`
  ```json
  {
    "account_id": "uuid-1",
    "cycle_date": "2026-10-01",
    "today": {
      "date": "2026-10-01",
      "account_user_id": "12345",
      "trigger": "scheduled",
      "status": "success",
      "started_at": "2026-10-01T01:00:01Z",
      "finished_at": "2026-10-01T01:00:04Z",
      "code": null
    },
    "history": [],
    "next_run_at": "2026-10-02T01:00:00Z"
  }
  ```

#### 单账号手动触发签到
- **路径**：`POST /api/accounts/{id}/checkin/run`
- **请求体**：
  ```json
  {
    "confirm_retry": false
  }
  ```
- **行为规范**：
  - 账号不存在返回 `404 Not Found`。
  - 当前周期已有成功记录直接返回 `200 OK` (`{"already_recorded": true}`)。
  - 当前周期处于 `unknown` 状态且未传 `confirm_retry: true` 时，拒绝并返回 `409 Conflict` (`checkin_retry_confirmation_required`)。
  - 校验通过后原子落盘 `running` 意图，返回 `202 Accepted` (`{"operation_id": "..."}`)，发起单次轻量 HTTP 签到请求。

---

## 6. 请求元数据与错误正文日志规范

为了在保护用户隐私的同时协助定位上游接口异常，系统引入专用的环形缓冲请求日志器。

### 6.1 日志数据结构 (RequestLog DTO)
```typescript
export interface RequestLogDTO {
  id: string; // 本地生成的 UUID
  timestamp: string; // 网关准入时的 ISO 8601 UTC 时间戳
  account_id: string | null; // 发起请求时快照中的账号本地 ID
  account_name: string | null; // 发起请求时快照中的账号用户名
  http_status: number | null; // 上游实际返回的 HTTP 状态码（null 表示未收到上游响应，如超时）
  error_body: string | null; // 错误响应正文原文（仅非 2xx），成功时严格为 null
  truncated: boolean; // 是否发生截断（超过 64 KiB 或传输异常早闭）
}
```

### 6.2 采集与存储安全规则
1. **成功请求（2xx）零正文原则**：
   - 对于所有返回 `2xx` 成功的请求，日志**仅记录**：时间戳、快照账号 ID/用户名、HTTP 状态码。
   - `error_body` 必须强制为 `null`，`truncated` 为 `false`。绝对不缓存或记录任何正常业务的请求/响应正文。
2. **非 2xx 错误正文保真捕获**：
   - 仅当上游返回非 2xx 状态码（如 400、401、403、429、500 等）时，系统捕获其原始响应正文的前 **64 KiB**（按字节截取，通过 `String::from_utf8_lossy` 转换为 UTF-8）。
   - 若错误正文超过 64 KiB 或读取遭遇未预期 EOF，置 `truncated: true`。
   - **用户授权与原始性**：由于上游服务可能在错误信息中回显敏感片段，此原始错误正文属于用户调试所需保留的明文。前端在渲染时必须将其放入 `<pre>` 标签作为**纯文本渲染，严禁作为 HTML 解析**以杜绝 XSS 风险。若上游自身回显了 Key 或敏感内容，系统依用户选择保留原始错误正文，不额外脱敏。
3. **零敏感输入记录**：
   - 日志系统严禁采集客户端请求体、URL 查询参数、HTTP 请求头（尤其是 `Authorization`、`Cookie`）、密码及 `root_key`。
4. **无假冒状态码**：
   - 若连接建立前发生本地超时或 DNS 解析失败，`http_status` 记为 `null`，严禁伪造本地生成的 502/504 充当上游状态码。
5. **流式传输与背压零干扰**：
   - 错误正文捕获使用旁路小缓冲区，流式传输（SSE）数据块直接转发给客户端，日志系统绝不缓存整个 SSE 流，严禁阻塞上游数据向客户端投递。
6. **存储配额与非阻塞环形缓冲**：
   - 内存维护容量为 256 的有界无阻塞通道；磁盘文件 `data/request-logs.json` 采用 `0600` 私有权限，大小严格限制在 **1 MiB** 以内，最多存储 **1000** 条记录（沿用既有实现上限示范值）。
   - 存储超限时自动淘汰最旧条目，并递增 `dropped_count` 计数器。

### 6.3 查询接口契约
- **路径**：`GET /api/request-logs?limit=100`
- **鉴权**：管理 Session Cookie，强制 `Cache-Control: no-store`
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
        "error_body": "{\"error\":{\"message\":\"Rate limit exceeded\",\"type\":\"requests\"}}",
        "truncated": false
      }
    ],
    "dropped_count": 0
  }
  ```

---

## 7. `/v1/models` 模型列表精确过滤规范

为了防止客户端调用上游已知不支持或失效的特定模型，网关对 `/v1/models` 实施离线白名单清洗。

### 7.1 过滤触发前置条件
仅当满足以下全部条件时才执行模型列表重构：
1. 请求方法为 `GET`，路径严格匹配 `/v1/models`。
2. 上游响应状态码为 `200 OK`。
3. 上游响应为有效 JSON 对象，且包含 `data` 数组。
4. 原始响应体总字节数 $\le 2\text{ MiB}$。

### 7.2 精确剔除的 13 个模型 ID
系统严格按字符串精确匹配并剔除以下 13 个模型标识符（大小写敏感），不使用泛正则表达式：
1. `gpt-5-codex`
2. `claude-3-5-haiku-20241022`
3. `claude-3-5-sonnet-20241022`
4. `claude-3-7-sonnet-20250219`
5. `claude-haiku-4-5-20251001`
6. `claude-opus-4-1-20250805`
7. `claude-opus-4-20250514`
8. `claude-opus-4-5-20251101`
9. `claude-opus-4-6`
10. `claude-opus-4-7`
11. `claude-sonnet-4-20250514`
12. `claude-sonnet-4-5-20250929`
13. `gemini-2.5-pro`

### 7.3 报文重构与安全标头处理
1. **结构保真**：
   - 除了剔除上述 13 个模型条目外，`data` 数组中其余模型的排列顺序、字段属性及顶层其他元数据字段（如 `object: "list"`）必须完全保真，不做字段重排。
   - 严禁对模型名称进行别名替换（no alias/rename），严禁在网关层校验或拦截后续推理请求中的模型参数。
2. **请求标头清洗**：
   - 向转发 `GET /v1/models` 请求时，剥离客户端的条件请求标头（`If-None-Match`、`If-Modified-Since`）与范围标头（`Range`），并强制注入请求头 `Accept-Encoding: identity`，确保接收未压缩明文。
3. **响应标头清洗**：
   - 在向客户端返回修改后的 JSON 响应体前，**必须彻底移除失效的响应标头**：`Content-Length`、`ETag`、`Digest`、`Content-MD5`、`Content-Encoding`。由网关 HTTP 框架依据修改后的实际字节重新生成正确的 `Content-Length`。
4. **异常降级规范**：
   - 若上游响应超过 2 MiB、JSON 解析畸变（malformed）或上游强行下发非 identity 压缩编码，网关向客户端返回 `502 Bad Gateway`（错误码 `models_response_invalid` 或 `models_response_too_large`）；在请求日志中记录上游实际返回的状态码 200，`error_body` 记为 null。
5. **其它路径透明穿透**：
   - 除 `GET /v1/models` 以外的所有 `/v1/*` 接口（如 `/v1/chat/completions`），保持纯透明代理：原样透传 HTTP 方法、URL 参数、请求正文、HTTP 状态码及 SSE 双向流式二进制分块。

---

## 8. 容器构建与私有 GHCR 工作流规范

项目提供标准 Docker 容器化支持与 GitHub Actions 自动化工作流。

1. **工作流触发与发布语义**：
   - 仅在私有仓库 `main` 分支发生代码 push 时自动触发构建。
   - 流程中严禁引入人工 `workflow_dispatch` 必填参数，严禁要求配置额外的专用 PAT（全流程使用系统内建的 `${{ secrets.GITHUB_TOKEN }}` 鉴权）。
2. **单镜像确定性 (Same Image ID)**：
   - 工作流必须在同一步构建出唯一镜像（`image ID`），并在该镜像内直接运行完整的容器级验证测试。
   - 测试通过后，将**同一个已验证的 Image ID** 打上标签并推送至私有 GHCR：
     - `ghcr.io/<owner>/anyrouter-manager:main`
     - `ghcr.io/<owner>/anyrouter-manager:<commit-sha>`
3. **闭源专有依赖合规与红线**：
   - 镜像内包含的修补版 CloakBrowser 二进制属于其专有独立许可条款范围，用户仅依据“Cloud Container Internal Use”条款在个人内部私有容器中运行。
   - **安全红线**：严禁在公开（Public）仓库运行工作流并公开推送镜像。
4. **真实 CI 状态严谨界定**：
   - 当前在本地环境仅完成 Dockerfile 语法与工作流配置文件的 18 项静态 mock 与 lint 检查。
   - 本地主机未安装 Docker Engine，远端私有 GitHub 仓库与 Actions 实际 runner 尚未运行；文档严禁声称远端镜像已发布或集群已验证。

---

## 9. 关键集成验证矩阵 (Verification Matrix)

在进入集成提交前，各责任模块必须对照以下矩阵进行针对性用例覆盖：

| 验证项与测试场景 | 核心检验标准与断言依据 | 责任角色 (Owner) |
| :--- | :--- | :--- |
| **Legacy 状态平滑迁移** | 无 `version` 的旧版 `Active` JSON 能被正确反序列化，生成 UUID、保持 credentials 与 key 并设为 route；若写入故障完整保留旧文件原貌。 | **Backend Fixer** |
| **路由快照连接隔离** | 模拟并发长时间流式请求，中间管理员触发 `POST /api/accounts/{id}/route` 变更路由，在途连接继续稳定接收旧账号数据流，后续新请求立即使用新账号。 | **Backend / Gateway** |
| **固定时槽排程防溢出** | 添加第 64 个账号或将间隔设置为过大数值导致 $\text{offset}_{N-1} \ge 1440$ 时，接口严格返回 `422 schedule_overflow`，状态回滚。 | **Backend / Checkin** |
| **失败与未知克制重试** | 模拟签到返回 500、超时或 WAF 挑战，验证记录被正确标记为 `failed`/`unknown`，当前周期后续槽位不再进行自动重发。 | **Checkin Fixer** |
| **日志环形缓冲与截断** | 构造并发 2xx 流量与连续 500 报错，验证 2xx 零正文、非 2xx 保留最大 64 KiB 且 UTF-8 有效；压测 1200 条验证旧记录淘汰与 `dropped_count` 准确递增。 | **Backend / RequestLog** |
| **模型列表精确 13 过滤** | 注入包含 13 个目标 ID 及其他正常模型的上游 JSON，验证响应精确剔除 13 项、保留其余顺序与字段，且响应头 `Content-Length` 已重新计算。 | **Backend / ModelFilter** |
| **前端纯文本安全渲染** | 前端接收包含特殊 HTML 标签或脚本字符的 `error_body`，在 `<pre>` 中严格以纯文本呈现，无 DOM 注入风险。 | **Designer (Frontend)** |
| **单浏览器全局互斥** | 混合登录、会话校验与前置探针并发竞争时，验证单一信号量全程锁定，直至子进程完全被操作系统回收。 | **Browser Helper Fixer** |
| **私有容器镜像构建约束** | 验证 GitHub Actions 工作流无 dispatch 必填项、复用同一 image ID 进行测试与推送，无公共分发风险。 | **CI Fixer / Parent Agent** |
