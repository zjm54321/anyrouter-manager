# AnyRouter Manager 内存基准测试报告 (Host Memory Baseline)

本文档记录 AnyRouter Manager 在 WSL2 本机宿主环境下的物理内存占用基准测试数据，测量基于本地回环（Loopback）无外部请求的测试夹具，旨在为后续容器化资源限制与内存优化提供客观数据支撑。

> **基准数据属性声明**：本文档记录的 6 轮基准测试（2,448 个采样点，Release 模式空闲 PSS 4.70 MiB、浏览器活跃 PSS 552.41 MiB / 观测峰值 556.30 MiB、退出后回落至 4.74 MiB）属于 **Phase A 实施前的宿主环境基线夹具测量数据**，并非 Phase A 实施后的重新测量值，亦非真实容器环境下的限制测量。严禁将应用程序单进程空闲 4.7 MiB RSS 误作为容器内存限制。

---

## 1. 测试方法与环境配置

- **测量环境**：Linux 6.18.33.2-microsoft-standard-WSL2 x86_64（宿主非容器环境，无外部网络请求，无真实 WAF 与真实账号）。
- **测量轮次与采样**：共执行 6 轮串行测试（3 轮 Debug 模式，3 轮 Release 模式），累计采集 2,448 个采样点；采样周期中位数为 200.096 ms，最大采样间隔 200.709 ms；`/proc/[pid]/smaps` 读取错误数为 0。
- **进程树监控范围**：统计归属于当前任务的全部唯一 PID，覆盖 Rust 后端（`anyrouter-manager-backend`）、`unshare` 容器包装器、`uv`、Python 登录适配器、Playwright Node 驱动、修补版 Chromium（`cloakbrowser-chrome`）及其脱离 PGID 的 Crashpad 守护进程（独立 PID 命名空间捕获）。观测到的最大并发 PID 数量为 15，对应 **1 个浏览器实例树**（包含 Zygote、GPU、网络、渲染及 Crashpad 等进程），而非 15 个独立浏览器。
- **业务场景模拟**：后端启动并托管已编译的 React 前端静态资源，预热 30 秒后进入 10 秒空闲基线；随后通过本地回环夹具发起模拟登录并保持 20 秒稳定活跃期，校验 Cookie 与资料后关闭浏览器；最后采集 20 秒登录后空闲状态。
- **指标定义**：
  - **PSS (Proportional Set Size)**：将多进程共享的物理内存按进程数均摊后与私有内存相加。因 Chromium 架构大量共享只读二进制与系统库，PSS 能真实反映物理内存占用。
  - **RSS (Resident Set Size)**：各进程常驻内存简单相加，存在对共享页面的严重重复计算（Double-counting）。
  - **排除项与局限性**：数据未计入未映射的页面缓存（Page Cache）、内核套接字/SLAB 内存及容器运行时底噪；采样间隔外的亚 200ms 瞬态峰值可能未被捕获；因 WSL2 宿主 `init.scope` 共享，无法提取纯净的单任务 cgroup `memory.current`。

---

## 2. 核心基准测量数据 (Main Baseline Matrix)

所有数值单位均为 **MiB**（1 MiB = 1,048,576 字节）。均值为 3 轮独立运行的时间加权均值，方括号内为各轮次实测极值区间。

| 编译模式 (Profile) | 测量窗口 (Window) | PSS 加权均值 (MiB) | PSS 观测峰值 (MiB) | RSS 加权均值 (MiB) | RSS 观测峰值 (MiB) | 单轮采样数 |
| :--- | :--- | :---: | :---: | :---: | :---: | :---: |
| **Debug** | **Backend Idle (启动后空闲)** | 11.75 | [11.75 – 11.76] | 13.65 | [13.64 – 13.65] | 50 |
| **Debug** | **Browser Active (浏览器活跃 20s)** | 558.94 | [561.41 – 562.14] | 977.82 | [982.01 – 983.95] | 100 |
| **Debug** | **Post-login Idle (退出后空闲)** | 11.79 | 11.79 | 13.77 | 13.77 | 100 |
| **Release** | **Backend Idle (启动后空闲)** | 4.70 | [4.70 – 4.71] | 6.59 | 6.59 | 50 |
| **Release** | **Browser Active (浏览器活跃 20s)** | 552.41 | [554.58 – 556.30] | 970.82 | [975.50 – 978.11] | 100 |
| **Release** | **Post-login Idle (退出后空闲)** | 4.74 | 4.74 | 6.72 | [6.71 – 6.72] | 100 |

### 启动与瞬态补充说明
- **后端冷启动阶段 (前 30s 预热)**：Debug 模式 PSS 均值 11.60 MiB（峰值 11.62 MiB）；Release 模式 PSS 均值 4.58 MiB（峰值 4.57 – 4.58 MiB）。
- **浏览器拉起阶段 (Browser Startup 瞬态)**：Debug 模式观测峰值 PSS 为 [531.40 – 538.77] MiB；Release 模式观测峰值 PSS 为 [526.76 – 555.73] MiB。

---

## 3. 数据分析与容器规格建议

1. **PSS 与 RSS 的巨大差距**：
   Chromium 多进程架构导致各渲染与工具进程重复映射主程序代码与系统动态库。在 Release 活跃期，RSS 加权值高达 ~971 MiB（峰值 ~978 MiB），而实际 PSS 仅为 ~552 MiB（峰值 ~556 MiB）。评估实际主机物理负荷应以 PSS 为准。
2. **退出后的内存回收确定性**：
   在浏览器完成流程并退出后，得益于专有 PID 命名空间与生命周期管理，派生进程完全被内核回收。Release 模式下系统内存立即回落至 **4.74 MiB PSS**，证明不存在浏览器孤儿进程滞留或常驻泄露。
3. **私有容器内存限制（Memory Limit）考量**：
   虽然 Release 模式下 PSS 峰值约为 **556 MiB**，但切勿将容器内存限制（如 Docker `--memory`）直接设定为 556 MiB，更不能将单进程空闲 4.7 MiB 误作为容器内存上限。容器规格必须留出安全边际，涵盖容器内系统运行时底噪、未映射页面缓存（Page Cache）、网络 I/O 缓冲区以及加载外部挑战页面时的瞬态波动。建议评估容器硬限制时不低于 1 GiB ~ 1.5 GiB。

---

## 4. 架构现状与计划中优化 (Architecture & Optimization Roadmap)

### 4.1 当前已实现的架构优化
- **单并发排队互斥锁与全局信号量**：后端在账户操作时引入互斥锁与全局单一浏览器信号量，从唤起到完全确认回收全程受控，杜绝并发启动多个浏览器进程。
- **混合登录 (HTTP-First Login)**：已落地。优先使用轻量级 HTTP 方式直接 POST 登录；仅当识别到上游已知 WAF 挑战阻断时，才单次回退启动无头浏览器；遇到业务失败、5xx、超时或未知 HTML 时绝不启动浏览器。
  - **内存表现界定**：该项优化带来的收益体现在**显著减少常规登录时唤起 Chromium 的频次**（在非 WAF 场景下免除 ~550 MiB 的浏览器物理开销），但当必须唤起浏览器穿透 WAF 时，**并不会降低 Chromium 运行时的单次拉起物理峰值**（峰值仍维持在 ~552–556 MiB PSS 范围；基准数字仅为 Phase A 实施前宿主本地测试夹具数据：Release 模式空闲 PSS 4.70 MiB、活跃均值 552.41 MiB、最高 556.30 MiB、退出后回落至 4.74 MiB，严禁冒充容器限额或新版本实测）。
- **浏览器仅限登录防护阶段使用**：无头浏览器严格仅用于初始登录与 WAF 阶段；后续的查余额、读已有 Key、签到接口以及 `/v1` 网关代理与流式传输，由 Rust 轻量异步 HTTP 处理，完全不唤起浏览器。

### 4.2 容器与生命周期管理机制
- **容器监督进程 (Supervisor) 方案**：在 `container_mode = true` 下引入 Rust subreaper 监督进程方案管理 helper 进程树，配合标准容器 PID namespace 与 `tini` 回收子进程，替代宿主环境下依赖复合 `unshare` 命名空间的机制，无需特权，该机制需待后续 CI 环境实测验证。
- **Helper 运行时代价评估**：Python helper 占用的 PSS 仅数十 MiB，活跃期内存的 95% 以上均被 Chromium 进程树（~540+ MiB）占用。因此不强制要求重构为 Rust 原生 helper，仅语言重写无法实质性改变 Chromium 占大头的事实。
- **明确不使用 `--single-process`**：Chromium 的单进程模式在官方被标为极不稳定且存在内存泄露与安全沙箱崩溃风险，本项目明确不采用此方案。

---

## 5. 复现命令与数据来源说明

本次基准测试使用的自动化采集与汇总脚本为临时调试工具，位于 `/tmp/opencode/memory-baseline-run/`，并非代码库持久提交的工具。所有测试数据均基于无账号的本地回环夹具，不含任何真实凭证或上游网络记录。

### 临时复现命令（依赖临时目录）
```sh
umask 077
nix develop path:/home/ming/anyrouter-manager#browser --command python -B /tmp/opencode/memory-baseline-run/measure.py
python -B /tmp/opencode/memory-baseline-run/summarize.py
```

- 完整原始采样数据、进程清单与二进制校验哈希参见临时文件 `/tmp/opencode/memory-baseline-run/report.json` 与 `provenance.json`。
- 后续如需固化该测试能力，可由后续任务将测量夹具迁移至 `tools/` 目录下。
