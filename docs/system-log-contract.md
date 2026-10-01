# AnyRouter Manager 系统日志与日志配置技术契约 (System Logs & Log Settings)

本文档定义 AnyRouter Manager 系统事件日志（System Logs）、日志全局配置（Log Settings）、双日志架构分流、运行时热重载过滤、持久化裁剪、UI 交互规范及验证矩阵。供后端（Backend Fixer）、前端（Designer / Frontend Fixer）及集成验证者作为冻结接口契约遵守。

> **基线与状态声明**：
> 1. 本契约建立在多账号与网关增强技术契约（`docs/multi-account-contract.md`）基础之上。当前系统日志 API 与全局配置已完全落地，并通过本地 HTTP 6 组集成检查（settings、filter、clear、restart、perms 等全部通过）；双日志交互面板已在前端整装集成并实测通过。
> 2. 系统日志子系统旨在提升运维可观测性与异常定位能力。线上级别测试证实：早期刷新失败被精确记录，修复后实际刷新成功；当前日志与 `known_challenge` 及 `task_start`/`task_finish` 等固定元数据的关联已通过测试。明确不声明 TRACE 能捕获 Python helper 每行代码，不提供二次 IPC 终态快照。
> 3. 前端多账号与运维面板已集成系统日志双选项卡；登录错误提示规范收敛至居中弹窗（Modal / Dialog），不再使用右上角常驻浮标（Toast）。

---

## 1. 双日志体系架构分流 (Dual-Log Architecture)

系统严格区分两类关注点完全不同的日志存储，禁止将两类日志混写或相互污染：

| 维度 | 网关请求日志 (Gateway Request Logs) | 系统事件日志 (System Event Logs) |
| :--- | :--- | :--- |
| **关注核心** | 外部客户端对 `/v1/*` 推理接口的代理请求元数据与上游异常响应 | 后端守护进程内部的业务状态机、凭据校验、生命周期、WAF 阶段与排程事件 |
| **触发范围** | 仅由网关 HTTP 请求与 SSE 流准入/结束触发 | 后端各业务模块（登录、刷新、签到、路由、配置持久化、服务启停）显式记录 |
| **正文策略** | • 2xx 成功：仅记录时间戳、账号名/ID 与状态码（**零正文**，不代表流完成）<br>• 非 2xx 异常：**保真记录前 64 KiB 原始正文**（依据用户选择保留原文排错，上游若回显敏感信息按原文留存，前端 `<pre>` 纯文本渲染） | **严格禁止任意自由文本或原始异常正文 Dump**。<br>详细性主要来自强类型枚举、阶段耗时（`elapsed_ms`）、关联 ID 与安全诊断布尔值；TRACE 日志策略禁止新增采集凭据，设计上不记录密码或敏感 Token（但不作第三方依赖永不泄漏的绝对承诺） |
| **存储文件** | `data/request-logs.json` (上限 1,000 条 / 1 MiB) | `data/system_logs.json` (上限 10,000 条 / 8 MiB) |
| **内存队列** | 容量 256 / 1 MiB 有界非阻塞通道 | 容量 256 / 1 MiB 有界非阻塞通道 |

---

## 2. 日志全局配置规范 (`LogSettings`)

### 2.1 配置结构定义

全局日志配置独立保存于 `data/log_settings.json`（权限 `0600`，父目录 `0700`），格式版本为 1：

```typescript
export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

export interface LogSettingsDTO {
  level: LogLevel;                  // 当前生效的全局日志过滤级别，默认 "info"
  system_retention_days: number;   // 系统日志保留天数，整数 1..90，默认 7
  request_retention_days: number;  // 网关日志保留天数，整数 1..90，默认 7
}
```

### 2.2 运行时热重载与事务回滚 (Hot-Filter & Rollback)
1. **Tracing 运行时整合**：
   - 基于 `tracing 1` 与 `tracing-subscriber 0.3` 构建专用 Layer，目标范围严格限定于本项目代码命名空间（`anyrouter_manager` 等），严禁对第三方依赖开启全局无序 Debug 输出。
2. **持久化优先与热重载原子性**：
   - 管理员通过接口修改配置时，系统**必须先完成本地 `log_settings.json` 的原子写入与落盘校验**。
   - 仅在磁盘写入成功后，才将新的日志级别热应用（Hot-Apply）至运行时的 Subscriber Filter；若磁盘 I/O 失败，立即回滚内存中的过滤级别，向客户端返回错误并保持原配置不变。
3. **安全诊断级别行为**：
   - 当配置为 `debug` 或 `trace` 级别时，新拉起的任务将激活安全诊断记录（`LoginDiagnostics`）；TRACE 策略禁止新增采集凭据，设计上不记录账号密码、完整 API Key、管理会话 Cookie 或上游 HTML 页面（但不作第三方依赖永不泄漏的绝对承诺）。

---

## 3. 结构化系统事件契约 (`SystemEvent`)

系统日志通过严格的数据契约提供细粒度的定位线索，拒绝不可预测的非结构化自由字符串。

### 3.1 事件数据模型

```typescript
export interface SystemEventDTO {
  id: string;                      // 本地生成的 UUIDv4 字符串
  timestamp: string;               // ISO 8601 UTC 时间戳（支持 Z 或 +00:00）
  level: LogLevel;                 // 事件所属日志级别
  event: SystemEventKind;          // 固定事件种类枚举
  operation_id: string | null;     // 关联的后台任务 ID（如适用）
  account_id: string | null;       // 关联的本地账号 UUID（如适用）
  stage: SystemStage | null;       // 当前操作所处的具体执行阶段
  elapsed_ms: number | null;       // 当前阶段耗时（毫秒，非负整数）
  http_status: number | null;      // 关联的上游 HTTP 状态码（100..599，如超时则为 null）
  reason: SystemReason | null;     // 阶段结束或失败的结构化原因代码
  diagnostics: SafeLoginDiagnosticsDTO | null; // 安全登录诊断快照（仅在有诊断时存在）
}

export type SystemEventKind =
  | "account_login"
  | "account_save"
  | "account_refresh"
  | "key_select"
  | "key_reveal"
  | "route_switch"
  | "checkin_scheduled"
  | "checkin_manual"
  | "checkin_slot_catchup"
  | "system_startup"
  | "system_shutdown"
  | "log_settings_updated"
  | "logs_cleared"
  | "storage_io";

export type SystemStage =
  | "http_login"            // 直接发起 HTTP POST 登录
  | "waf_detected"          // 检测到已知 WAF HTML 页面
  | "browser_permit_wait"   // 等待单一浏览器全局信号量
  | "browser_spawn"         // 拉起 Helper 进程与浏览器
  | "helper_result"         // 等待 Helper 执行结束并解析结果
  | "cleanup"               // 清理子进程与命名空间
  | "timeout"               // 触发全流程统一截止时间
  | "storage_write"         // 状态文件原子落盘
  | "route_snapshot";       // 路由快照提取

export type SystemReason =
  | "ok"
  | "credentials_rejected"
  | "waf_challenge"
  | "browser_failed"
  | "timeout"
  | "network_error"
  | "schedule_overflow"
  | "storage_failure"
  | "already_done"
  | "slot_missed";

export interface SafeLoginDiagnosticsDTO {
  version: 1;
  phase: string;
  page: string;
  login_requested: boolean;
  login_status: number | null;
  login_json: boolean;
  login_success: boolean | null;
  self_requested: boolean;
  self_status: number | null;
  self_json: boolean;
  self_success: boolean | null;
  self_id_valid: boolean;
  user_state_ready: boolean;
  failure_request: string;
  exception: string;
}
```

### 3.2 进程边界与最后阶段捕获 (Stage & Hang Boundary)
- **Helper 单结果约束**：当前 Python Helper 在进程退出时向标准输出（stdout）输出单行最终 JSON 结果，无逐行 IPC 进度推送通道。
- **真实挂起阶段记录**：当登录过程发生不可逆的底层硬挂起（如无头浏览器卡死或进程死锁），Rust 侧的全局截止时间（统一 Timeout）将被触发。Rust 监控线程记录**最后明确进入的阶段**（如 `stage: "browser_spawn"` 或 `stage: "helper_result"`），并附带 `reason: "timeout"` 与耗时 `elapsed_ms`，**绝不伪造虚假实时的每行进度日志**。

---

## 4. API 路由与清理契约

所有管理接口遵循 `/api` 路径，强制 Session Cookie 鉴权、响应强制下发 `Cache-Control: no-store`。所有写操作（`PUT`/`POST`）强制校验请求的 `Origin` 与 `Host`。

### 4.1 获取日志配置
- **路径**：`GET /api/log-settings`
- **鉴权**：管理 Session Cookie
- **响应**：`200 OK`
  ```json
  {
    "level": "info",
    "system_retention_days": 7,
    "request_retention_days": 7
  }
  ```

### 4.2 更新日志配置 (同步)
- **路径**：`PUT /api/log-settings`
- **请求体**：必须包含全部 3 个字段：
  ```json
  {
    "level": "debug",
    "system_retention_days": 14,
    "request_retention_days": 7
  }
  ```
- **参数校验**：
  - `level` 必须为有效枚举；
  - `system_retention_days` 与 `request_retention_days` 必须为整数且落在 `1..90` 范围内。
- **处理规范**：先持久化写入磁盘 `0600` 文件，成功后动态更新运行时过滤器，并立即按新天数执行一次数据裁剪。
- **响应**：`200 OK`，返回更新后的完整 `LogSettingsDTO`。失败返回 `400 Bad Request` 或 `500 Internal Server Error`。

### 4.3 查询系统事件日志
- **路径**：`GET /api/system-logs`
- **参数**（均可选）：
  - `limit`：返回数量，整数，默认 100，钳制在 `1..1000`；
  - `level`：最低级别阈值（如传 `warn` 则返回 `warn` 与 `error` 级别的事件）；
  - `account_id`：按账号本地 UUID 精确过滤；
  - `operation_id`：按异步操作 UUID 精确过滤。
- **响应**：`200 OK`
  ```json
  {
    "items": [
      {
        "id": "sys-log-uuid-1",
        "timestamp": "2026-10-01T10:00:05.123Z",
        "level": "error",
        "event": "account_login",
        "operation_id": "op-login-uuid",
        "account_id": null,
        "stage": "helper_result",
        "elapsed_ms": 30045,
        "http_status": null,
        "reason": "timeout",
        "diagnostics": {
          "version": 1,
          "phase": "launch",
          "page": "other",
          "login_requested": false,
          "login_status": null,
          "login_json": false,
          "login_success": null,
          "self_requested": false,
          "self_status": null,
          "self_json": false,
          "self_success": null,
          "self_id_valid": false,
          "user_state_ready": false,
          "failure_request": "none",
          "exception": "timeout"
        }
      }
    ],
    "dropped_count": 0
  }
  ```

### 4.4 清空系统事件日志
- **路径**：`POST /api/system-logs/clear`
- **请求体**：空 JSON `{}`
- **行为规范**：
  - 引入**代际栅栏机制 (Generation Barrier)**：记录当前清空操作的启动时间戳/序列代际，在此之前已排队或在途的旧事件，在处理时直接丢弃，严禁在清空完成后重新回填到存储中。
  - 清空磁盘日志文件与内存缓存。写入失败时返回 `503 Service Unavailable`，保留原有数据。
- **响应**：`204 No Content`

### 4.5 清空网关请求日志
- **路径**：`POST /api/request-logs/clear`
- **请求体**：空 JSON `{}`
- **行为规范**：同样通过代际栅栏丢弃在清空时刻之前启动、而在清空后才结束的在途长连接请求日志，防止旧流结束时回填数据。
- **响应**：`204 No Content`（写入故障返回 `503 Service Unavailable` 并保留数据）。
- **注意**：日志清空操作不可逆，但**绝对不影响任何账号凭据、API Key、余额或签到历史数据**。

---

## 5. 存储安全、生命周期与背压隔离

1. **多文件路径防别名冲突**：
   - 系统涉及的全部文件路径在初始化阶段必须完成物理路径别名解析与冲突校验：
     `config.toml`、`state_path` (`account.json`)、`checkin_state_path` (`checkin.json`)、`request_log_path` (`request-logs.json`)、`system_log_path` (`system_logs.json`)、`log_settings_path` (`log_settings.json`)。
   - 任何两个路径不得通过软链接、相对路径或 `..` 跃迁指向同一物理文件。在同一私有目录存放多个独立命名的状态文件为合法配置。
2. **容量配额与非阻塞通道**：
   - 系统事件单个大小预估 $\le 4\text{ KiB}$。
   - 内存维护容量为 **256** 的有界非阻塞通道（约 1 MiB 内存缓冲）；超限或队列繁忙时直接丢弃新事件并原子递增 `dropped_count`，**严禁阻塞业务请求或挂起网关推理流**。
   - 磁盘文件上限为 **10,000 条** 记录与 **8 MiB** 物理字节大小。
3. **保留天数与安全裁剪策略**：
   - 裁剪触发时机：服务启动加载时、后台每分钟定时器（Periodic Tick）以及配置被更新时。
   - 严格删除时间戳早于当前时刻减去 `retention_days` 的过期记录。
   - **时间漂移保护**：若遇到时间戳位于未来的记录（如系统时钟漂移或重设），**不按时间天数删除**，但依然受条数（10,000）与文件大小（8 MiB）的硬性配额约束。
4. **服务停机排空 (Shutdown Flush)**：
   - 接收到系统终止信号（SIGTERM/SIGINT）时，日志工作协程执行屏障排空，并设置 **硬性 2 秒超时**；若 2 秒内无法完成刷盘则强制退出，绝不无限期阻塞主进程退出。
5. **日志写入失败安全防递归**：
   - 若日志落盘发生 I/O 错误，内部日志汇（Sink）仅输出限频（Rate-Limited）的固定标准错误错误码（如 `system_log_sink_failed`），严禁触发二次递归日志记录，避免发生日志死锁。

---

## 6. 前端界面与错误交互规范 (UI & Error UX)

前端多账号与运维面板需遵循以下交互设计（已在前端完成集成并通过真实验收）：

1. **日志面板双选项卡布局 (Tabs Layout)**：
   - 日志管理页面提供两个主选项卡：
     - **网关请求日志 (Gateway Logs)**：展示请求时间、账号名称、状态码、截断标记，点击非 2xx 条目弹出查看 64 KiB 原始错误响应文本（纯文本 `<pre>` 呈现）。
     - **系统事件日志 (System Logs)**：展示时间、级别徽标（Level Badge）、事件类型、关联 `operation_id` / `account_id`、执行阶段（Stage）、耗时（`elapsed_ms`）及原因代码。
2. **统一配置控制栏 (Settings Controls)**：
   - 页面顶部提供统一的操作栏：
     - 日志级别下拉选择器（Error / Warn / Info / Debug / Trace）；
     - 系统日志保留天数（1..90）与网关日志保留天数（1..90）输入控件；
     - 独立的“清空系统日志”与“清空请求日志”按钮（二次确认弹窗防误触）。
3. **登录失败错误交互规范（居中弹窗，非右上角浮标）**：
   - **交互收敛至 Modal / Dialog**：当登录操作失败时（如遇到超时、WAF 无法穿透或密码错误），错误信息必须在**居中的对话框（Modal Dialog）**中明确展示，内容包括错误分类码、安全错误描述以及对应的 `operation_id`。
   - **跳转至关联系统日志**：弹窗内提供可选按钮“查看诊断日志”，点击后自动关闭弹窗、跳转至“系统事件日志”选项卡并自动应用 `operation_id` 过滤条件，方便用户直接定位底层阶段与耗时。
   - **禁止行为**：严禁将登录失败以右上角常驻浮标（Sticky Toast / Notification Banner）的形式简单弹框，避免关键排错信息被自动遮挡或随页面滚动丢失。

---

## 7. 关键集成验证矩阵 (Verification Matrix)

| 验证项与测试场景 | 核心检验标准与断言依据 | 责任角色 (Owner) |
| :--- | :--- | :--- |
| **日志级别热更新与回滚** | 修改级别并模拟磁盘只读故障，验证内存中的 tracing 过滤级别被安全回滚，日志配置原样保留。 | **Backend Fixer (LogSettings)** |
| **TRACE 级别密钥哨兵** | 将级别置为 `trace` 并执行模拟登录与 Key 查询，扫描持久化 JSON 与输出，验证无密码、API Key、Cookie 或 HTML 泄漏。 | **Backend Fixer (SystemLog)** |
| **底层硬挂起末尾阶段记录** | 构造模拟 Helper 挂起或不退出场景，验证 Rust 超时触发后，准确记录最后进入的 `stage`（如 `browser_spawn`）与 `reason: "timeout"`。 | **Backend / Browser Helper** |
| **保留天数与未来时间保护** | 构造过去 10 天与未来 1 天的记录，验证 7 天策略下精确裁剪旧记录，未来时钟偏差条目不被误删但受条数配额约束。 | **Backend Fixer (SystemLog)** |
| **代际栅栏防在途回填** | 构造长时间执行的后台任务，在执行中途调用清空接口，验证任务完成后其迟到的旧事件被栅栏直接丢弃。 | **Backend Fixer (SystemLog)** |
| **高并发与磁盘故障非阻塞** | 填满 256 队列并挂起磁盘写入，向 `/v1` 并发注入请求，验证网关代理继续保持正常传输，仅系统日志 `dropped_count` 递增。 | **Gateway / Backend** |
| **登录错误弹窗与查看日志联动** | 模拟登录失败，验证前端渲染居中弹窗、展示 `operation_id`，点击后正确跳转系统日志并过滤显示对应条目。 | **Designer (Frontend)** |
| **停机 2 秒硬超时** | 模拟日志写盘阻塞，向主进程发送 SIGTERM，验证服务在 2 秒内强行退出，不产生僵尸进程。 | **Backend / Parent Agent** |
