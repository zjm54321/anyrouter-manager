# AnyRouter Manager 签到技术契约与规范

本文档定义 AnyRouter Manager 定时与手动签到功能的系统架构、管理 API 契约、周期计算语义、状态持久化机制、上游交互约束及测试验证门控。供后端（Backend Fixer）与前端（Frontend Fixer）实现时遵守。

> **前置事实声明与免责边界**：
> 1. 本项目签到上游交互逻辑参考公开开源实现（见 [millylee/anyrouter-check-in](https://github.com/millylee/anyrouter-check-in/blob/3b16662142c68703ea3e79c77c06a8cf6bba7990/utils/config.py)），**非 AnyRouter 官方工具，未经官方背书**。
> 2. 用户明确指明 AnyRouter 签到周期以**北京时间每天 08:00 为新一轮签到起点**，而非自然日零点。该规则源自用户设定，单点现场观察未独立验证日切规律；2026-10-01 现场观察报告（详见 `docs/checkin-observation.md`）已证实登录后自然触发与手动 POST 均指向 `POST /api/user/sign_in`；本轮样本实测支持严格布尔值 `success: true`，且 message 匹配到“签到成功”（脱敏报告未完整保存逐字原文，不作为官方契约保证）；重复签到（`already_done`）响应未实测，白名单当前视为空；当次观察到的 25 美元为页面提示文案，不写死为系统未来恒定奖励值。
> 3. 本文档仅为技术架构设计契约，不包含产品代码实现，不进行外部网络探测，严禁记录或落盘用户真实密码。

---

## 1. 核心设计原则与周期语义

1. **签到周期语义 (Cycle Date Semantics)**：
   - 签到周期以北京时间（Asia/Shanghai，固定 UTC+8 无夏令时）每日 08:00 为起始点。
   - 周期日期定义：`cycle_date = date(local_now[Asia/Shanghai] - 8h)`，数学上严格等价于当前 `UTC date`。
   - 周期日期 `D` 覆盖的时间区间为：`[D 08:00 CST, D+1 08:00 CST)`。
   - 记录中的 `date` 字段统一保存 `YYYY-MM-DD`（即 `cycle_date`），前端展示时明确标注为“当前签到周期”。
   - 唯一性排重主键二元组为：`(account_user_id, cycle_date)`。
2. **意图优先写 (Write-Intent First)**：
   - 向上游发起带副作用的签到 POST 请求前，必须先原子落盘 `status: "running"` 意图记录。若落盘失败则禁止发送网络请求，防止产生失控副作用。
3. **极度克制的自动重试**：
   - 当前周期内只要产生过任何触发尝试（无论 `failed`、`unknown` 还是 `success`），**当前周期内绝不进行任何自动二次重试**。
   - `unknown` 状态代表上游可能已执行但本地未确认，人工重试必须携带显式确认标志，避免重复签到导致风控异常。
4. **无密码自动登录**：
   - 系统坚持“密码仅登录瞬时驻留内存、绝不落盘”的既有安全边界。会话 Cookie 过期时，签到任务直接标记失败并等待用户手动在 UI 重新登录，**绝不通过任何后台自动化机制重新输入密码**。
5. **轻量无头调度与受管登录防护一致性**：
   - 定时调度器仅执行轻量级时间检查，不唤起浏览器。
   - **日常受管登录拦截页面自动签到**：现场观测证实线上控制台（SPA）在登录成功后会自动发起 `POST /api/user/sign_in`。为了让签到行为严格作为受控的计划任务由管理器显式发起、记录并落盘意图，日常管理器登录（`managed login`）通过浏览器守卫主动拦截页面自动发起的 `POST /api/user/sign_in`，避免因日常管理操作导致状态漏记或与计划调度冲突（这与线上页面真实自动发起签到并不矛盾）；只读校验模式（`session_verify`）亦严格禁止一切写操作。
   - **执行路径唯一性**：只有显式手动触发（`manual`）或定时计划触发（`scheduled`）时，本地管理器才在原子落盘 `status: "running"` 意图后由 Rust 轻量 HTTP 发起单次 `POST /api/user/sign_in`，降低“登录自动签到却漏记状态”或“同周期双重触发”的概率。
   - **WAF 对抗与重发限制**：发起签到前若纯 HTTP 请求 `GET /api/user/self` 遇到已知 WAF 阻断，仅允许拉起一次只读 `session_verify` 模式辅助进程以确立会话与身份，确认后再发起单次 HTTP POST；若 `POST /api/user/sign_in` 遇到超时、5xx 或 WAF 挑战，统一标记为 `unknown`，绝不自动二次重发。

---

## 2. 管理 API 路由契约

所有接口遵循 `/api` 基础路径，强制管理端 Session Cookie 鉴权、`Origin` / `Host` 安全校验及 `Cache-Control: no-store`。

### 2.1 获取签到状态与历史
- **路径**：`GET /api/checkin`
- **鉴权**：管理 Session Cookie 鉴权，强制校验 `Origin` / `Host`，响应头包含 `Cache-Control: no-store`
- **成功响应**：`200 OK`
  ```json
  {
    "settings": {
      "enabled": false,
      "time": "09:00",
      "timezone": "Asia/Shanghai",
      "reset_time": "08:00"
    },
    "cycle_date": "2026-10-01",
    "today": null,
    "history": [],
    "next_run_at": "2026-10-02T01:00:00Z"
  }
  ```
  - `cycle_date`：服务端统一计算的当前签到周期起始日期（`YYYY-MM-DD`），前端 UI 严禁依据客户端设备自然日展示或计算签到状态。
  - `today`：返回归属于当前激活账号、对应当前签到周期 `cycle_date` 的记录对象（`CheckinRecord`），无记录时为 `null`。
  - `history`：返回按当前激活账号过滤后的历史摘要数组（`CheckinRecord[]`），上限 180 条，按周期倒序排列。
  - `next_run_at`：下一次计划调度的 ISO 8601 UTC 时间戳。若 `settings.enabled === false` 或未配置激活账号，返回 `null`。

### 2.2 更新定时签到配置
- **路径**：`PUT /api/checkin/settings`
- **鉴权**：管理 Session Cookie 鉴权，强制校验 `Origin` / `Host`，响应头包含 `Cache-Control: no-store`
- **请求体**：
  ```json
  {
    "enabled": true,
    "time": "09:30"
  }
  ```
  - `time` 必须严格满足 24 小时制 `HH:MM` 格式 (`^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$`)。
  - 允许配置 `08:00` 之前的时刻（如 `02:00`），该时刻指代周期内次日凌晨（北京时间 D+1 02:00，仍属于周期 D）。
- **成功响应**：`200 OK`，返回更新后的完整状态对象 `CheckinFullStatus`（结构同 `GET /api/checkin`）。
- **失败响应**：
  - `400 Bad Request`：时间格式非法。

### 2.3 手动触发签到 (异步任务)
- **路径**：`POST /api/checkin/run`
- **鉴权**：管理 Session Cookie 鉴权，强制校验 `Origin` / `Host`，响应头包含 `Cache-Control: no-store`
- **请求体**：
  ```json
  {
    "confirm_retry": false
  }
  ```
- **处理与状态码规范**：
  - **已成功/已签到 (200)**：若当前账号在当前签到周期内已存在 `status: "success"` 或 `status: "already_done"` 记录，直接返回 `200 OK`，不发起重复请求：
    ```json
    {
      "already_recorded": true
    }
    ```
  - **启动异步任务 (202)**：校验通过，返回 `202 Accepted`，由全局异步状态机接管：
    ```json
    {
      "operation_id": "op-checkin-uuid"
    }
    ```
  - **任务并发冲突 (409)**：当前系统已有运行中的后台任务（如登录、刷新或其他签到）：
    ```json
    {
      "error": {
        "code": "operation_in_progress",
        "message": "当前已有任务正在执行，请稍后"
      }
    }
    ```
  - **未知状态需确认 (409)**：当前周期记录为 `unknown` 且请求未显式携带 `confirm_retry: true`：
    ```json
    {
      "error": {
        "code": "checkin_retry_confirmation_required",
        "message": "当前周期签到结果处于未确认状态，重试可能导致重复提交，请确认后重试"
      }
    }
    ```
  - **未配置账号 (409)**：当前系统未配置已激活的 `active` 账号：
    ```json
    {
      "error": {
        "code": "account_not_configured",
        "message": "未配置可用账号，无法执行签到"
      }
    }
    ```

---

## 3. 数据模型定义 (DTO Specifications)

```typescript
export interface CheckinSettings {
  enabled: boolean;          // 默认 false
  time: string;             // 24小时制 "HH:MM"，默认 "09:00"
  timezone: "Asia/Shanghai"; // 只读固定值，本地统一策略时区 (UTC+8)
  reset_time: "08:00";       // 只读固定值，上游签到重置与周期起点（北京时间 08:00）
}

export type CheckinTrigger = "manual" | "scheduled";

export type CheckinStatus =
  | "running"       // 意图落盘，正在请求中
  | "success"       // 上游明确返回成功（本轮样本实测支持 success: true，message 匹配到“签到成功”）
  | "already_done"  // 上游明确返回已签到（未独立实测，白名单暂视为空）
  | "failed"        // 明确执行失败（未产生不可控副作用）
  | "unknown";      // 状态未确认（可能已产生副作用）

export type CheckinCode =
  | "ok"
  | "already_checked_in"
  | "session_expired"
  | "network_error"
  | "upstream_5xx"
  | "upstream_challenge"
  | "schema_mismatch"
  | "intent_persist_failed";

export interface CheckinRecord {
  date: string;               // 签到周期日期 cycle_date，格式 "YYYY-MM-DD"，服务端统一计算
  account_user_id: string;    // 上游稳定用户数字 ID 字符串
  trigger: CheckinTrigger;
  status: CheckinStatus;
  started_at: string;         // ISO 8601 UTC
  finished_at: string | null; // ISO 8601 UTC，running 状态为 null
  code: CheckinCode | null;   // 安全固定枚举，禁止注入原始响应或敏感信息
}

export interface CheckinFullStatus {
  settings: CheckinSettings;
  cycle_date: string;          // 格式 "YYYY-MM-DD"，服务端统一计算当前周期日期，禁止前端按设备自然日计算
  today: CheckinRecord | null; // 当前周期归属于当前 active 账号的记录
  history: CheckinRecord[];    // 按当前 active 账号过滤后的历史记录（至多 180 条）
  next_run_at: string | null;  // 下一次计划执行时间 (ISO 8601 UTC)
}
```

### 3.1 既有异步操作结构扩展 (`BackgroundOperation`)
在 `docs/api-contract.md` 基础之上扩展：
- `kind`: 新增 `"checkin"`。
- `phase`: 新增 `"checking_in"`。

---

## 4. 状态持久化机制与崩溃恢复

1. **独立存储文件与配置项**：
   - 运行时配置支持可选参数 `checkin_state_path`，若未指定则默认位于 `state_path` 所在目录同级的 `checkin.json`。
   - 文件权限严格维持 `0600`（父目录 `0700`），持久化写入采用临时文件写入后 `rename` 的原子替换策略。
   - **严禁修改既有 `data/account.json` 结构**。
2. **容量边界与记录隔离**：
   - **磁盘持久化容量**：磁盘文件 `checkin.json` 保持保留最近 180 条全账号记录（`all accounts` 最近 180 条）。
   - **激活账号数据隔离**：管理端 API 接口在查询与返回 `history` 数组时，必须以当前激活账号的 `account_user_id` 进行过滤，最多返回归属于当前账号的最近 180 条历史记录，确保单账号切换时历史数据相互隔离。
   - **周期内更新合并**：同一账号在同一签到周期内若进行人工重试，直接更新/替换当前周期记录，不无限追加冗余日志。
3. **意图写入与断电/崩溃恢复流程**：
   ```
   [触发签到]
       │
       ▼
   1. 原子写入 status="running", finished_at=null 到 checkin.json
       ├─ 写失败 ──> 终止流程，直接返回错误，不发起网络请求
       └─ 写成功 ──> 2. 向 AnyRouter 发起上游 POST 请求
                         │
                         ├─ 网络超时/5xx ──> 更新 status="unknown", finished_at=now
                         ├─ 明确失败 ──────> 更新 status="failed", finished_at=now
                         └─ 明确成功 ──────> 3. 原子更新 status="success", finished_at=now
                                               └─ 若写持久化失败 ──> 标记 status="unknown"
   ```
4. **服务重启恢复 (Restart Recovery Gate)**：
   - 后端启动加载 `checkin.json` 时，若发现存在残留的 `status: "running"` 记录，**必须在调度器启动前将其强制修正为 `status: "unknown"` 并原子持久化**。
   - 启动后若当前签到周期没有任何签到记录且满足配置，调度器方可执行补发逻辑；若当前周期已存在任何记录（包括恢复出的 `unknown`），当前周期自动调度静默挂起。

---

## 5. 定时调度与重试控制策略

1. **下一次触发时间计算 (Next Run Calculation)**：
   - 假设当前周期起始于北京时间日期 `D` 08:00 CST：
     - 若配置时间 `time` >= `08:00`：计划执行时刻为北京时间日期 `D` 的 `time`。
     - 若配置时间 `time` < `08:00`：计划执行时刻为北京时间日期 `D+1` 的 `time`（仍在周期 `D` 内）。
   - 若计划时刻已过，且当前周期未曾尝试，则触发补签；否则计划时刻推演至下一周期（`D+1` 的相应时刻）。
2. **轻量 Tick 调度器**：
   - 后台由 Tokio 定时器以轻量周期（建议 30 秒）检查 `next_run_at`。
   - 调度器不保持常驻浏览器，不占用重型资源。
3. **补发与单周期频次硬限制**：
   - **停机补发 (Catch-up)**：服务在设定时间之后启动且当前周期尚无任何尝试记录时，触发一次补签。
   - **历史周期不回溯**：严禁对过去已过周期的缺失记录进行补发。
   - **防并发锁**：若触发时刻全局操作锁被占用，调度器放弃本轮，下一个 Tick 重新探测，不写 `running` 意图。
   - **周期自动唯一性**：当前周期无论成功、失败或未确认，**后续 Tick 坚决不再自动触发**。08:00 CST 周期轮转后自动解开限制。
4. **跨 08:00 边界处理 (Cross-08:00 Guard)**：
   - 若签到请求在 07:59:59 CST 发起，响应在 08:00:01 CST 返回，因跨越了重置边界且上游记账归属未知，系统标记 `unknown`，并同时锁定跨越的前后两个周期，防止自动化程序立即在 08:00 后重复自动调用。
   - 请求若跨越自然日零点（00:00 CST），因处于同一签到周期内，不触发特殊异常，正常按当前周期记账。系统不承诺上游端 Exactly-Once。

---

## 6. 上游调用规范与 WAF 对抗约束

### 6.1 身份预校验
发起签到前，必须先在纯 Rust 环境下向 `GET https://anyrouter.top/api/user/self` 发送请求以验证当前会话可用性并确认 `account_user_id`。
- 若返回 401 或会话失效，直接标记 `status: "failed", code: "session_expired"`，终止流程。
- 若返回上游防火墙已知的 JS 挑战页面，允许拉起一次仅用于会话保持的只读 helper（`session_verify` 模式），此时 helper 严格拦截所有写请求，杜绝误触发签到。

### 6.2 签到网络请求规格
- **目标地址**：`POST https://anyrouter.top/api/user/sign_in`（现场实测确认为无 query 的 POST 接口）。
- **请求体与标头约定**：
  - 空 Body 及特定请求头主要源自第三方开源工程实现参考，现场脱敏观察报告证实了目标路径为 `POST /api/user/sign_in` 且无 query 参数。
  - 请求体：必须为空 JSON `{}` 或空 Body。
  - 请求头建议：
    - `Cookie`: 携带已保存的有效 Session Cookie。
    - `New-Api-User`: 携带当前账号的 `upstream_user_id` 字符串。
    - `Origin`: `https://anyrouter.top`
    - `Referer`: `https://anyrouter.top/console`
    - `X-Requested-With`: `XMLHttpRequest`
- **重定向控制**：严格禁止跟随 3xx 重定向（`redirect: manual`）。

### 6.3 响应判定准则 (Success Criteria)
- **成功判定**：HTTP 状态码必须为 `2xx`，响应体必须为合法 JSON 且明确包含 `success: true`。
  - **实测验证**：基于 2026-10-01 现场观察报告（`docs/checkin-observation.md`），本轮样本实测支持严格布尔值 `success: true`，且 message 匹配到“签到成功”（脱敏报告未完整保存逐字原文，不作为官方契约保证）。
  - 严禁通过 `ret: 1`、`code: 0` 或 HTML 中包含特定字符作为成功依据。
- **已签到判定 (`already_done`)**：
  - 目前上游关于“今日已签到”的官方错误码与文本白名单尚未经过真实样本实测，**白名单暂视为空**。
  - 系统严禁从任意模糊匹配的 HTML/JSON 文本臆测已签到状态，坚决不为了探测已签到响应而在单周期内人为发起二次签到。
- **未确认判定 (`unknown`)**：
  - 遇到 HTTP 5xx、网络中断、连接重置、不符合协议契约的非预期 JSON 或 HTML 挑战页面，统一归入 `unknown`。
  - **重要约束**：若 `sign_in` POST 本身遇到 WAF 挑战，**禁止自动拉起无头浏览器重新 POST**（因为 POST 请求可能已被上游处理），必须保持 `unknown` 并交由用户人工判断。
- **风控保护**：禁止使用任何自动打码平台（Captcha solver），禁止任何密集重试（Hammering）。

---

## 7. 验证门控测试用例清单 (Verification Test Gate)

后端实现已通过以下自动化 Mock 测试用例集（用例均基于本地 Tokio 异步 Mock，不代表对真实 AnyRouter 线上行为的担保）：

1. `test_checkin_cycle_calculation_and_advance`：验证北京时间 08:00 边界计算，`07:59` 与 `08:01` 分属不同 `cycle_date`，且 `cycle_date` 等价于该时刻的 UTC 日期。
2. `test_checkin_write_intent_before_post`：验证在执行网络 POST 之前，`checkin.json` 中已成功持久化 `running` 状态；模拟写文件失败时，验证上游 Mock 服务器未收到任何 POST 请求。
3. `test_checkin_server_restart_running_recovery`：模拟进程异常退出留存 `running` 状态，重启后验证记录被自动恢复为 `unknown`，且调度器在当前周期不发起二次自动请求。
4. `test_checkin_catchup_current_cycle_only`：验证停机跨周期重启时，仅在当前周期未尝试时补签一次，绝不回溯历史缺失周期。
5. `test_checkin_cross_0800_boundary_locks_both`：模拟跨越 08:00 CST 边界的超时请求，验证其标记为 `unknown` 并锁定关联周期。
6. `test_checkin_single_concurrency_lock`：当存在活跃的登录或刷新任务时，验证 `POST /api/checkin/run` 同步返回 `409 operation_in_progress`。
7. `test_checkin_manual_unknown_requires_confirmation`：当前周期存在 `unknown` 记录时，验证默认手动触发返回 `409 checkin_retry_confirmation_required`；仅在传递 `confirm_retry: true` 时返回 202。
8. `test_checkin_cross_account_isolation`：切换激活账号后，验证 `today` 仅返回当前激活账号的周期记录，排重键为 `(account_user_id, cycle_date)`。
9. `test_checkin_session_expired_fails_safely`：模拟上游 Cookie 过期，验证签到安全失败并提示会话失效，绝不尝试自动读取或提交密码。
10. `test_checkin_helper_session_verify_blocks_all_writes`：验证辅助进程在 `session_verify` 模式下仅执行 GET 探测，拦截所有 POST 请求以防误签到。
