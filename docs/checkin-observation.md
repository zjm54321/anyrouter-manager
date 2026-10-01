# AnyRouter 线上签到现场观察报告 (Checkin Observation Report)

本文档记录 2026-10-01 针对 AnyRouter 控制台自然签到行为的端到端现场观察记录。测试在用户授权隔离环境下执行（第二授权示例账号），产生 1 次登录与 1 次签到真实副作用。审计数据源自脱敏报告 `/tmp/opencode/anyrouter-checkin-observation-ucqwld5y-complete.json`（报告不含账号、密码、Cookie 或完整标头）。

---

## 1. 现场时序基准 (Factual Timeline)

时序严格区分**等待输入阶段**与**浏览器运行耗时**，记录 UTC 与北京时间（Asia/Shanghai，UTC+8）：

- **09:48:30 UTC / 17:48:30 CST** (`observed_utc`)：
  - 监控脚本完成初始化并启动。
  - *等待输入说明*：脚本启动后挂起约 4 分 14 秒，等待通过关闭回显的标准输入（stdin）接收真实凭据。此等待时间并非浏览器运行耗时。
- **09:52:44.896 UTC / 17:52:44.896 CST** (`input_received`)：
  - 关闭回显的标准输入接收凭据完毕，开始进入自动化流程；输出报告脱敏。
- **09:52:45.010 UTC / 17:52:45.010 CST** (`launch`)：
  - 无头浏览器实例启动。
- **09:52:45.522 UTC / 17:52:45.522 CST** (`navigation`)：
  - 导航至上游入口并加载页面。
- **09:52:54.296 UTC / 17:52:54.296 CST** (`form_wait`)：
  - 表单渲染就绪，期间记录到前端请求 `/api/notice` (200) 与 `/api/status` (200)。
- **09:52:54.877 UTC / 17:52:54.877 CST** (`submit_trial`)：
  - 提交动作可用性检查（actionability check），检测到页面浮动公告遮挡指针事件（`TimeoutError: intercepts_pointer_events`），尚未执行实际点击提交。
- **09:52:58.896 UTC / 17:52:58.896 CST** (`dismiss_notice_close`)：
  - 触发关闭遮挡公告（`notice_dismissed: true`）。
- **09:52:59.118 UTC / 17:52:59.118 CST** (`submit`)：
  - 遮挡消除后，执行表单提交点击。
- **09:52:59.155 UTC / 17:52:59.155 CST** (`request_evidence: login` 发起时间)：
  - 发起 `POST /api/user/login` 请求（带有页面 query，具体值未采集）；随后捕获 HTTP 200，`success: true`，登录成功。
- **09:52:59.560 UTC / 17:52:59.560 CST** (`request_evidence: sign_in` 发起时间)：
  - 控制台 SPA 页面在登录成功后自动发起 `POST /api/user/sign_in`（无 query）。
  - 随后捕获 HTTP 200 JSON 响应，服务端响应头包含 `Date: 09:53:00 UTC`。
  - 脱敏报告记录 `success: true`，`message` 字段类型为 string 且匹配到“签到成功”短语（原文未完整保存，示例结构为 `{"success": true, "message": "<原文未保存；匹配到“签到成功”>"}`，非逐字返回）。
- **09:53:00.009 UTC / 17:53:00.009 CST**：
  - 页面提示气泡匹配到“签到成功”，并提示本轮奖励金额 25 美元（当次提示，不写死为系统未来恒定奖励值）。
- **09:53:06.530 UTC / 17:53:06.530 CST**：
  - 页面请求 `GET /api/user/self`，返回 HTTP 200，`success: true`，含正整数 ID 与 quota。
- **09:53:06.532 UTC / 17:53:06.532 CST**：
  - 安全过滤拦截 1 次非白名单敏感只读请求（`sensitive_read: 1`, `path: "other"`）。
- **09:53:07.018 UTC / 17:53:07.018 CST** (`verified_console`)：
  - 会话与 SPA 身份状态确认就绪。
- **09:53:07.024 UTC / 17:53:07.024 CST** (`passive_automatic_observation`)：
  - 被动观察窗口期未额外发起签到，亦未观察到第二次签到请求。
- **09:53:19.125 UTC / 17:53:19.125 CST** (`observation_complete` & `cleanup`)：
  - 观测结束，浏览器 Context 与进程关闭成功，父命名空间退出（`cleanup: true`）。

---

## 2. 证据归纳与指标对比

| 维度 | 本轮采集证据 | 说明与来源划分 |
| :--- | :---: | :--- |
| **登录尝试与结果** | 发起 1 次，HTTP 200 | `POST /api/user/login` 发起时间 09:52:59.155 UTC |
| **签到尝试与结果** | 发起 1 次，HTTP 200 | `POST /api/user/sign_in` 发起时间 09:52:59.560 UTC |
| **签到触发机制** | `automatic_on_page` | SPA 页面自然自动触发，非脚本主动点击 |
| **接口路径与参数** | `POST /api/user/sign_in`，无 query | 本次实测采集确证 |
| **响应格式实测** | JSON 对象，`success: true` | 脱敏报告记录 `success: true` 并匹配“签到成功”；非官方规范保障 |
| **请求体与标头** | 暂未采集逐字明文 | “空 Body、指定 Header”属第三方开源参考，非本次实测采集证明 |
| **阻断与审计** | `sensitive_read: 1` | 跨域/重定向/重复提交均为 0，拦截 1 次非白名单读取 |

---

## 3. 关键事实核验与工程决策

1. **早期测试失败归因**：
   - 早期失败尝试因自动化选择器误选、公告遮挡点击及 query 规则误判所致，这些早期失败尝试均未把登录或签到 POST 发到上游；修正后的本轮为 1/1。
2. **页面自动签到与受管登录**：
   - 实测证实 SPA 页面登录后自动请求 `/api/user/sign_in`，与第三方脚本直接 POST 指向相同上游接口。
   - AnyRouter Manager 受管登录（`managed login`）通过安全守卫拦截页面自动 sign_in（功能实现中），旨在降低登录意外触发导致漏记状态或同周期双重触发的概率，收敛至管理器显式受控调度。
3. **周期与重复签到局限**：
   - 08:00 CST 周期划分来自用户给定规则，单点实测未独立验证跨日切行为。
   - 重复签到（`already_done`）响应尚未实测，白名单当前视为空，不为探测故意重复签到。
