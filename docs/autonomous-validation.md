# 自主交付与验证跟踪报告

本报告记录 AnyRouter Manager 自主交付阶段的真实功能验证、前端重构进展、回归测试矩阵及基础设施预检事实。

---

## 一、授权基线与门控阶段

- **授权状态**：用户已全自动授权端到端交付（含私有 GitHub 仓库创建、GHCR 镜像构建与 Homelab GitOps 部署），覆盖原手工确认门控。但**真实功能测试与安全/权限门控仍属强制前提**，严禁虚构通过。
- **代码与服务基线**：`/home/ming/anyrouter-manager`，本地 Git 基线 `main 353282a`（12 commits），本地改动未提交，无远端 remote。服务运行于 `http://127.0.0.1:18880`（会话 `pty_b7a52a89`），保留原 root 密钥与 3 个本地账号。
- **阶段门控流程**：
  - **Phase 1**：UI 修复与验收 + 核心功能真实核验 ──► Oracle Gate 1
  - **Phase 2**：创建私有仓库 + 代码审计推送 + 验证 GHCR 构建 ──► Oracle Gate 2
  - **Phase 3**：Homelab 声明式配置 + 现有权限适配部署 ──► Oracle Gate 3

---

## 二、测试回归与真实核验事实

### 2.1 确定性回归测试矩阵（离线无网络）
- **Cookie 修复根因**：原保存的 3 个账号各含 4 个 Cookie（3 个过期），致旧解析判定为 malformed。Rust 增加危险字段校验并过滤过期项，无 expiry 规范化为 `-1`；可用 session 为空返回 `session-expired` 而非 `bad-input`。Python 离线验证每账号各 1 有效 Cookie 均通过。
- **自动化测试套件**：
  - 后端：133 项 unit + 9 项 integration 全部通过；2 项授权 probe 与 1 项原生浏览器测试默认 `ignored`（此前已单独跑通）。
  - 浏览器助手：82 项 Python 测试 + 14 项 native session fixture 测试通过。
  - 前端：117 项单元/组件测试、类型检查与构建全部通过；包含修复 RevealDialog 身份时序（persistent id/revision ref、layoutEffect 失效生成与状态清理）新增的 15 项测试（此前复现 12 项失败，修复后 15 项全过）及 17 项 hook race 回归测试。
  - CI 配置：13 条纯测试 exact 各 1 项通过；静态检查 18 项、工作流配置 6 项（18 种组合）及 hadolint/actionlint/shellcheck 全部通过（无实际 Docker 运行）。

### 2.2 真实业务功能验证证据
依据脱敏真实操作报告，记录受限环境核验结果：

| 功能验证项 | 状态 | 真实证据与观察事实 | 边界与限制 |
| :--- | :--- | :--- | :--- |
| **账号刷新** | **通过** | 报告 `idp7bbvh.json`：前两账号串行刷新各 POST 1 次（6724 ms / 5930 ms），阶段经历 `reading_account` → `listing_keys` → `done`，更新 revision/时间，原 route/key/第三账号不动；各记录 1 次 `known_challenge`（INFO 级别，无 spawn 日志）。 | 本轮未用真实密码登录；不声称日志证明精确浏览器实例。 |
| **每日签到** | **通过** | 报告 `tws8rux2.json`：针对此前 preflight failed 的第 2 账号发起当前周期 1 次 POST，操作状态 `succeeded`，持久化状态更新为 `success`；生产环境匹配 2xx 与 `success: true`。 | 本轮未触发 `already_done`，未发起二次重复请求；全程无额外探针。 |
| **网关路由与模型** | **历史验证** | 历史实测 13 项黑名单模型过滤生效（返回 3 项可用模型），请求日志准确映射测试账号。 | 属历史验证记录；本轮未发起模型请求，不代表付费推理已测。 |
| **前端界面 UI** | **通过** | • 报告 `uq3ngeb0/report.json`：登录解锁、4 主页面导航（overview/accounts/logs/systemlogs/settings）、系统日志选项卡、6 列数据表、原生仅设置页时间配置、弹窗重开字段清理、单次刷新成功且 pending 状态与行保留可见、revision/时间更新、账号/设置/路由/Key 保持不变、退出登录通过。<br>• 报告 `o3wyg0y1.json`：真实用户 UI 单次登录与保存通过（单次初始文档请求无全页重载，阶段流转 `authenticating` → `reading_account` → `listing_keys` → `done`，密码清理，候选态可见，已保存账号 upsert 成功后弹窗正常关闭，排程/Key/路由/顺序保持不变，注销成功）。 | save pending 过程过快未采样到（由确定性测试覆盖，不作为状态保证）；UI 守卫未观测 helper 上游流量，不伪造上游凭据证明。此前报告 `36ihze3u` 记录的 Modal 重开断言失败作为排查历史客观留存，不抹杀曾经失败的事实。 |

---

## 三、部署预检与基础设施审计

| 基础设施项 | 审计现状 | 观察事实与客观阻断 | 应对准则 |
| :--- | :--- | :--- | :--- |
| **GitHub 仓库** | 待创建 | `gh` 已认证用户 `zjm54321`；目标私有仓库 `anyrouter-manager` 目前 404（尚未创建）。 | Phase 2 门控后创建私有仓库并推送代码。 |
| **GHCR 镜像凭据** | **权限阻断** | 现有 OAuth Token 缺少 `read:packages` 范围，查询 Packages API 返回 **403 Forbidden**。 | 部署尚未开始；严禁通过将镜像设为公开来绕过权限。不可用短期 Actions Token 替代集群拉取凭据。 |
| **Homelab GitOps** | 待对齐 | 本地 `fe91032` 落后 `origin/main d276b18` 5 个提交（0 ahead，工作区干净）。 | Git push Flux 可声明式创建资源，无需本机 kubectl admin；配置前需快照同步。 |
| **集群访问与特权** | **权限阻断** | SSH 可连接 `192.168.1.127`（普通用户 1000）；无权读取 k0s kubeconfig/admin.conf；`doas -n` 需密码，无 sudo。 | 缺乏 live 集群动态读取权限与长期 private GHCR pull secret；宿主 `/var/lib/homelab`（0711）普通用户无法 mkdir。 |
| **安全工具与存储** | 就绪 (待授权) | sops 3.13.3 与 age 1.3.2 工具可用；新服务 home 目录 PV 仅为候选方案，需后续 phase 合法例外。 | 尚未推送任何配置，未创建本地或远程目录。 |
| **节点硬件容量** | **容量风险** | 物理内存总量 3,814 MiB，当前可用内存仅 **758 MiB**，Swap 已用 601 MiB。 | 2 GiB limit 非硬性物理空闲要求，但 Chromium 峰值 PSS 达 ~552–556 MiB，内存裕量偏紧。 |

---

## 四、当前状态小结

1. **Phase 1**：后端测试全过（133 unit + 9 integration）、Helper 测试全过（82 python + 14 native）、前端测试全过（117 unit/build）；真实账号刷新、单次签到以及原生 UI 验收（整装四页+刷新+真实登录保存）均已取得通过证据；**已在第 2 次审查后正式通过 Oracle Gate 1 评审**。
2. **Phase 2 & 3**：私有仓库创建、GHCR 镜像构建与 Homelab 部署按序推进。客观记录 Packages 403 权限与集群内部 Secret 访问限制，不伪造凭据或公开镜像绕过。
