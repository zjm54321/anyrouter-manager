# AnyRouter Manager 多账号签到技术契约与规范

本文档定义 AnyRouter Manager 多账号固定时槽定时与手动签到功能的系统架构、管理 API 契约、周期计算语义、排程算法、状态持久化机制、上游交互约束及测试验证门控。供后端（Backend Fixer）与前端（Frontend Fixer）实现时遵守。

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
2. **多账号固定时槽排程 (Fixed Time Slot Scheduling)**：
   - 系统为单进程串行调度体系（非分布式系统）。所有在册账号按照添加至系统的严格顺序（索引 $i = 0, 1, \dots, N-1$）分配执行槽位。
   - 全局配置包含基准起始时间 `time`（`HH:MM`）与执行间隔 `interval_minutes`（整数，范围 $1 \le \text{interval\_minutes} \le 1440$，默认 30 分钟）。
   - **槽位偏移量计算规则**（相对于周期起点 08:00 的分钟偏移）：
     - 设用户配置时间对应的自然日分钟数为 $M = \text{hour} \times 60 + \text{minute}$。
     - 若 $M \ge 480$（08:00 至 23:59）：基准偏移 $\text{base\_offset} = M - 480$。
     - 若 $M < 480$（次日 00:00 至 07:59）：基准偏移 $\text{base\_offset} = M + 960$。
     - 账号 $i$ 在周期内的执行偏移量为：$\text{offset}_i = \text{base\_offset} + i \times \text{interval\_minutes}$。
3. **排程越界保护 (`schedule_overflow`)**：
   - 最后一个账号的执行偏移量必须满足 $\text{offset}_{N-1} < 1440$（24 小时内）。
   - 更新配置（`PUT /api/checkin/settings`）或添加新账号（`POST /api/accounts/save`）时，若造成排程溢出，系统在同一互斥锁下严格拒绝并返回 **`422 Unprocessable Entity`** (`schedule_overflow`)。
4. **意图优先写与重试限制**：
   - 向上游发起签到 POST 前，必须先原子落盘 `status: "running"` 意图记录；若落盘失败则禁止发送网络请求。
   - 某个账号的耗时或失败不推迟后续账号的预定槽位；当前周期内只要产生过自动尝试（无论是 `failed` 还是 `unknown`），**本周期内绝不再自动二次重试**。
   - `unknown` 状态手动重试必须携带 `confirm_retry: true`。
5. **忙碌排队与补偿运行 (Catch-Up)**：
   - 到达槽位时若系统全局 busy，忙碌结束后立即补偿运行（catch-up）当前周期内已过槽位的账号。
   - 调度器**绝不向前追溯补跑历史已终结周期的签到**。
   - 新加入账号若槽位已过，当前周期内尚未尝试的前提下获得 1 次当即补偿运行机会。
6. **轻量无头调度与受管登录防护**：
   - 定时调度器仅执行轻量级时间检查，等待期间处于异步休眠状态，**严禁持有任何互斥锁，绝不唤起或常驻浏览器**。
   - **日常受管登录拦截页面自动签到**：托管登录模式下通过浏览器守卫主动拦截页面自动发起的 `POST /api/user/sign_in`，将签到副作用严格收归计划任务控制；`session_verify` 模式严格只读。
   - 签到前若遇到已知 WAF 阻断，仅拉起一次只读 `session_verify` 确立会话后发起单次轻量 HTTP POST；若遇到超时、5xx 或挑战统一标记为 `unknown`，绝不自动重发。

---

## 2. 管理 API 路由契约

基础路径为 `/api`。全部接口强制 Session Cookie 鉴权、响应强制下发 `Cache-Control: no-store`。所有写操作强制校验请求的 `Origin` 与 `Host` 请求头。

### 2.1 获取全局签到状态与排程
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

### 2.2 更新全局定时签到配置
- **路径**：`PUT /api/checkin/settings`
- **请求体**：
  ```json
  {
    "enabled": true,
    "time": "09:00",
    "interval_minutes": 30
  }
  ```
- **响应**：
  - `200 OK`：返回更新后的全局状态。
  - `400 Bad Request`：时间或间隔参数格式非法。
  - `422 Unprocessable Entity` (`schedule_overflow`)：排程跨度超出 24 小时周期。

### 2.3 获取指定账号签到历史
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

### 2.4 单账号手动触发签到 (异步)
- **路径**：`POST /api/accounts/{id}/checkin/run`
- **请求体**：
  ```json
  {
    "confirm_retry": false
  }
  ```
- **响应**：
  - `200 OK` (`{"already_recorded": true}`)：当前周期已有成功或已签到记录。
  - `202 Accepted` (`{"operation_id": "..."}`)：成功排队发起单次 HTTP 签到。
  - `404 Not Found`：目标账号不存在。
  - `409 Conflict` (`checkin_retry_confirmation_required`)：当前周期为 unknown 且未传 confirm_retry。
  - `409 Conflict` (`operation_in_progress`)：全局已有任务在运行。

### 2.5 废弃的历史单账号端点
- 旧版 `POST /api/checkin/run` 直接返回 **`410 Gone`** (`deprecated_endpoint`)，禁止通过旧接口隐式触发签到。

---

## 3. 持久化存储与容量裁剪规范

1. **存储文件**：
   - 签到状态保存于 `data/checkin.json`（由配置项 `checkin_state_path` 指定，默认与 `state_path` 同目录），权限严格为 `0600`（父目录 `0700`），采用临时文件原子替换写入。
2. **容量边界与多身份隔离**：
   - 单账号保留最多 180 条历史记录；全系统历史记录总上限为 **11,520 条**；文件大小上限为 **8 MiB**。
   - **历史身份数与当前账号数区别**：系统当前支持最多 64 个活跃账号，但历史记录中出现过的不同 `account_user_id` 身份总数不受 64 限制（旧的第 65+ 个历史身份记录完全可被正常加载与保留，不与当前活跃账号数混淆）。
3. **记录保护与安全裁剪**：
   - 在历史记录接近上限进行容量修剪时，**严格保护以下记录绝不被删除**：
     - 当前签到周期的所有记录（`current cycle`）；
     - 上一签到周期的所有记录（`previous cycle`，用于日切对齐与去重）；
     - 未来的计划记录（`future cycles`）；
     - 处于运行中状态的记录（`status: "running"`）。
   - 仅对历史已终结、且早于上一周期的旧记录进行先进先出淘汰；过期的 `CycleAmbiguous` 记录在符合条件时可安全裁剪。
