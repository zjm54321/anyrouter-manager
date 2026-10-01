# Restricted AnyRouter browser helper

此目录是 Rust subprocess 的 Python stdin/stdout 登录适配器，仅允许固定
`https://anyrouter.top`。不负责部署浏览器、签到或余额请求。

## 环境与调用

从项目根进入 `nix develop`，然后在 `tools/browser-helper` 工作目录执行：

```sh
uv sync --frozen
uv run --frozen python -m browser_helper
```

已经同步依赖的解释器也可在本目录执行 `python -m browser_helper`
（例如 `.venv/bin/python -m browser_helper`）。依赖精确锁定：
`cloakbrowser==0.5.11`、`playwright==1.58.0`，完整传递依赖见 `uv.lock`。
`uv sync` 只安装 Python 包；不要执行 Playwright/CloakBrowser 浏览器安装命令。

调用者必须配置 `CLOAKBROWSER_BINARY_PATH` 为绝对路径，指向**已经可在当前
Nix/Linux 环境运行的 CloakBrowser patched 浏览器**。路径须存在且可执行；
helper 原样保留该环境变量，缺失时在 import wrapper 前返回
`browser_unavailable`，不会下载或回退。可执行权限检查不证明浏览器兼容性。
普通 Chromium 不是等效替代；真实 patched 浏览器兼容性另行门控。

父进程向 stdin 写一行 UTF-8 JSON（不要把密码放在命令行、环境变量或磁盘），
随后关闭 stdin：

```json
{"username":"<username>","password":"<password>","timeout_ms":60000}
```

仅接受这三个字段；`timeout_ms` 可省略（默认 60000），必须为整数
1000–120000，不会静默扩展时限。请求最大 16384 字节，用户名最大 320 UTF-8
字节、密码最大 4096 UTF-8 字节，禁止控制字符和重复 JSON 键。读首行有 5 秒
时限；一次进程只处理一个请求，不是多行服务。浏览器流 overall 时限包含启动、
导航、表单及 profile 验证，结束额外最多 2 秒用于 context/browser 顺序关闭。
Rust 应保持 subprocess 总时限并在超时终止进程组（包含浏览器子进程）；Python
超时是协作式取消，不能保证中断卡死的原生代码或阻塞的 wrapper。

stdout 严格输出一行 JSON，成功退出码 0，失败退出码 1：

```json
{"ok":true,"cookies":[{"name":"session","value":"<secret>","domain":"anyrouter.top","path":"/","secure":true,"http_only":true,"expires":-1}],"api_user":"42"}
```

`expires` 保留 Playwright 的 Unix 秒值（`-1` 表示 session cookie）。stdout
成功响应是敏感数据，Rust 不应记录它。失败仅返回固定错误码，无 cookie 字段：

```json
{"ok":false,"error":"browser_unavailable"}
```

错误码：`login_form_unavailable`、`challenge_or_block`、`timeout`、
`user_self_unverified`、`browser_unavailable`、`invalid_input`、`login_failed`。
进程入口将 fd 1/2 永久指向 `/dev/null`（退出前不恢复），仅保留不向子进程继承
的专用 fd 输出最终 JSON，并处理 partial write。浏览器、wrapper、Python logging、
原生缓冲及 exit hooks 的 stdout/stderr 均丢弃，不输出异常正文。不会截图、录屏、trace、保存 cookie 或创建
持久 profile；浏览器本身仍可能使用临时运行目录，关闭后由 Playwright 清理。

## 验证规则与限制

- fresh context，TLS 校验不关闭，service worker 禁用，所有跨源网络请求拒绝。
  没有有证据的第三方静态资源 allowlist，本轮不猜 CDN 域名。公开 `/login` 检查
  仅报告同源 `acw_sc__v2` 挑战脚本，未发现必要第三方 CDN 的证据；实际登录页
  静态资源完整性与可用性仍未验证。跨源资源依赖可能使登录不可用。
  每个同源请求均用 `route.fetch(max_redirects=0, max_retries=0)` 获取并 fulfill，
  不使用 `route.continue_()`；所有 HTTP 3xx（包括同源 redirect）均 abort。
  此保守策略避免 307/308 自动跨域重发密码 body，但也会拒绝正常重定向。
- 有限等待邮箱/用户名表单；仅点击公开流程的邮箱入口，没有人工 challenge 绕过。
  可见 challenge/checking-browser 或 403/429/503 被动等待，允许挑战自行消失并出现
  正常表单；只在 overall 时限耗尽且仍被阻断时返回 `challenge_or_block`。
  未知 DOM 返回 `login_form_unavailable`；不声称密码错误。每次填凭据和点击提交前
  要求当前页面为精确 HTTPS `anyrouter.top` 的 `/login`（不接受其它同源路径）。
- 在点击登录前注册本轮 response listener，保留提交时同步触发的 self 响应。
  只跟踪精确同源 `POST /api/user/login`；必须 HTTP 200 JSON `success: true`，
  `success: false` 返回 `login_failed`，不猜密码错误。无关 POST、可选资源失败或
  redirect 不推进/污染认证；主导航、login、self 关键网络失败仍终止。
  不通过固定计时器或单凭 login response 强制导航：优先保留 SPA 导航，只在安全
  页面、document ready 和 `localStorage.user.id` 正整数状态连续稳定且无 pending
  login 后，才允许同一 context 触发 `/console`。storage ID 只用于内部就绪判断，
  必须与 self ID 一致，绝不替代真实 profile 响应。监听精确 HTTPS 同源
  `/api/user/self`（包括拒绝跨源 redirect 链），必须 HTTP 200、JSON
  `success: true`、`data.id` 或 top-level `id` 为正整数（不接受 bool/float/string，
  最大 `2^63-1`）。console URL 本身绝不是登录证据。
- 只有确认 profile 且页面为安全 console URL 后才读取全部 context cookies，不按
  `/console` 过滤，因此保留 `Path=/api` 的 session cookie。再次核实当前页面
  仍是安全 console URL。仅保留精确 `anyrouter.top` / `.anyrouter.top` 域、合法
  cookie token 名和 RFC cookie-octet 值；拒绝 CR/LF、分号及其他注入字符。
  没有合格 cookie 则失败。Rust 仍须独立校验 domain/path/有效期和 Cookie header。
- 参考指定 commit 的 `utils/browser.py` / `checkin.py` selectors；并未验证实时
  真实账户或 WAF 登录。Nix patched 浏览器本地执行已过下述门控，不能视为真实登录通过。

## 可选失败诊断契约 v1

仅环境变量 `BROWSER_HELPER_DIAGNOSTICS=1` 开启；缺失、`0`、`true` 等均关闭。
stdout 仍为单行 JSON，只在失败附 `diagnostics`；成功不含诊断。
固定字段（没有账户、ID 值、header 值、URL、cookie、响应正文或 storage 内容）：

- `version: 1`；`phase`: `launch|navigation|form_wait|fill|submit|profile_wait|cookie_read|done`；
  `page`: `login|console|other`。
- boolean: `login_requested`, `login_json`, `self_requested`, `self_json`,
  `self_id_valid`, `self_user_header_present`, `user_state_ready`, `pending_login`。
- `login_status`, `self_status`: `null` 或整数 100–599；
  `login_success`, `self_success`: `null` 或 boolean。
- `self_id_type`: `absent|integer|string|other`；
  `failure_request`: `none|navigation|login|self|resource`；
  `exception`: `none|timeout|navigation_transient|network|unexpected`。

字段通过白名单验证构造，不透传 raw payload。Playwright TimeoutError 单独识别；
fill/submit/profile_wait 保留阶段。只在安全同源重试 evaluate 导航 context 瞬态，
最多三次；不重复登录提交，不 force click。可见表单还必须 enabled/editable。

## 离线测试

不安装浏览器、不访问 AnyRouter、不需要真实账号：

```sh
python -m unittest discover -s tests -v
# 或用锁定环境
uv run --frozen python -m unittest discover -s tests -v
```

测试覆盖模拟 browser/context/page、profile 失败不得读取 cookie、跨源 response/
redirect、ID 类型、cookie 过滤、JSON/重复键/大小限制、缺二进制禁止 import/下载、
strict stdout 和脱敏、challenge/DOM 变化、超时及关闭失败的资源回收。
回归另覆盖 307/308 no-follow（两个真实本地 HTTP origin、mock route.fetch，第二
origin 零请求）、非 login 路径禁止填密码/提交、早到 self、超过 5 秒慢登录、
瞬态 challenge、`Path=/api` cookie、libc printf 延迟 fflush/exit hooks 与 partial write。
本地 HTTP 测试不是实际 Playwright/浏览器 redirect 行为验证。

## 锁定依赖门控记录（2026-09-30）

- `nix develop --command uv sync --no-progress --frozen --project tools/browser-helper`
  已成功安装锁定依赖（Playwright wheel 下载约 10 分 30 秒），未安装或启动浏览器。
  初次 PTY 的进度输出触发 `fatal runtime error: assertion failed: output.write(&bytes).is_ok(), aborting`；
  改为无进度输出后同步成功，没有修改依赖或锁文件。
- 实际 `.venv/bin/python` 为 Python 3.14.7；metadata 确认
  `cloakbrowser==0.5.11`、`playwright==1.58.0`。真实 `import cloakbrowser`
  成功，`launch_async` 存在、可调用且为 coroutine function，**没有执行 launch**。
- 在 helper 工作目录执行
  `nix develop --command uv run --frozen python -B -m unittest discover -s tests -v`：
  37/37 离线 mock/local HTTP/subprocess 测试通过（14.056 秒）。这些 mock 测试
  不证明 Playwright 原生依赖或浏览器可用；wrapper import 单独检验，没有模拟 import。
- **初次运行环境门控阻断（后续已修复，见下节）**：真实 `from playwright.async_api import Browser, async_playwright`
  在导入 greenlet 时失败：
  `ImportError: libstdc++.so.6: cannot open shared object file: No such file or directory`。
  Python 包安装通过不等于 Python 3.14 下的完整 native runtime 已通过；需由 Nix
  环境 owner 声明/提供对应运行库，再重新验证 Playwright/greenlet import。
- patched 浏览器执行、真实浏览器 redirect 行为、实时登录页 DOM/WAF、真实账户登录
  仍未测试；不得据此声称浏览器兼容或挑战已可通过。
- 后续在 `nix develop .#browser` 中复验（不 launch）：Playwright/greenlet import
  仍报上述 `libstdc++.so.6` 缺失错误；同环境 locked unittest 37/37 通过
  （14.029 秒）。patched binary 已构建不代表 Python wheel 的动态库已配置。
  父门控另发现 Crashpad 可逃出 PGID，需 job cleanup；本 helper 不保证仅杀进程组
  能回收所有浏览器派生进程，本轮没有启动浏览器。

## 真实浏览器本地门控（2026-09-30）

`tests/local_browser_smoke.py` 为独立本地 fixture，不属于 unittest discovery，不运行
生产 `python -m browser_helper`，不读取账号凭据、不访问 AnyRouter。Nix 环境 owner
修复 native runtime 后，此脚本在实际 locked Python 3.14.7 / CloakBrowser 0.5.11 /
Playwright 1.58.0 环境使用以下 patched binary 通过：
`/nix/store/sv231g34qjil8r23fc43i1mnlmyvgzhn-cloakbrowser-chromium-146.0.7680.177.5/bin/cloakbrowser-chrome`。

在 helper 工作目录复用以下命令（必须保留 namespace；总超时建议 240 秒）：

```sh
HELPER_SMOKE_HOST_PIDNS="$(readlink /proc/self/ns/pid)" nix develop .#browser --command \
  unshare --user --map-current-user --pid --fork --kill-child=KILL --mount-proc -- \
  uv run --offline --frozen --no-sync python -B tests/local_browser_smoke.py
```

脚本拒绝与 host 相同的 PID namespace。私有 PID namespace 的 PID1 退出回收该
namespace 内派生进程（包括 setsid Crashpad），不能把普通 PGID 当等效清理。
仅输出安全断言 JSON，fd 1/2 永久丢弃，没有截图/trace/cookie 值日志。
脚本不新增 `--no-sandbox`；**wrapper 0.5.11 默认参数本身含 `--no-sandbox`**，
因此此测试不声称 Chromium sandbox 开启。禁止 wrapper 自动更新，指定已构建 binary；
浏览器外部代理指向关闭的 loopback 端口，仅放行本地 127.0.0.1，外域 DNS 映射失败；
Python audit 也拒绝非 loopback connect。

实测通过：
- 真实 `launch_async(headless=True)` 返回 Browser，new_context/page、about:blank JS、
  title 验证、context/browser close。
- 真实 route.fetch no-follow + fulfill 页面，document.cookie 与 HTTP Set-Cookie，
  context 全量 cookies 同时包含 `/` 和 `/api` path。
- 真实 Playwright 两个 loopback origin：POST 虚构 sentinel 的 307/308 均拒绝重定向，
  第二 origin 零请求。
- fixture 内临时替换 helper ORIGIN/safe_url/cookie-domain 验证（不改生产允许目标）：
  提交触发早到正整数 self 响应成功；6 秒慢提交完整结束后才导航 console，真实 self
  JSON 验证及 `/api` cookie 保留成功。均无真实账号或真实 WAF 的证明。

首次 smoke 的浏览器/cookie/redirect 已通过，但 helper fixture cookie 过滤的 test-only
safe_url 嵌套替换错误导致断言失败；修复 fixture 后全部通过，未修改生产 helper。
实时 DOM、真实账号登录、WAF 通过能力仍未验证。生产 Rust 隔离/timeout 集成由父门控负责。

## 真实模块入口诊断

在 helper 工作目录执行（建议父总时限 120 秒）：

```sh
nix develop .#browser --command uv run --offline --frozen --no-sync \
  python -B tests/module_entry_probe.py
```

独立 probe 创建 `/tmp/opencode/helper-local-entry-*` 临时目录及全新 `UV_CACHE_DIR`，
设置 `UV_NO_SYNC=1`，在私有 PID namespace 中真正执行
`uv run --offline --frozen --no-sync python -B -m browser_helper`。先用 `{}` 检查
`invalid_input`；再仅通过临时 sitecustomize fixture 替换本地 origin/domain 校验，
以虚构 sentinel 验证真实模块入口、wrapper launch、自身 profile/cookie/close。
不是生产可配置 URL，未修改生产 helper；真实浏览器保留 local-only 网络限制参数。

实测 `{}` 协议正确，真实模块入口返回成功单行 JSON、stderr 为空；binary 存在且
可执行、Nix node 存在、LD_LIBRARY_PATH 已继承、wrapper 预检查/import 通过。
launch 时 fd1/fd2 均为 `/dev/null`，专用输出 fd 不继承策略未阻断 Playwright
握手或浏览器启动；真实 self 正整数 ID、`/api` cookie path、browser close 均通过。
本次进程未配置 proxy 环境变量；因此没有证据把上游失败归因于 stdout 重定向、
空 uv cache、namespace 权限或 wrapper 启动，也不证明真实网站/WAF/profile 已可用。
probe 仅输出安全断言/固定错误码，不打印完整 cookies、账户、环境内容或 wrapper 日志。

## 认证状态修复回归（2026-09-30）

在相同 locked native 环境执行上面的 unittest、local_browser_smoke、module_entry_probe：
49/49 测试通过（16.561 秒）；两项真实浏览器 fixture 返回 `ok:true`。
既有 37 个测试保留并更新为真实认证顺序，新增 12 个回归（共 49）：HTTP200 应用失败、
无关 POST、SPA storage 延迟、optional image 302/失败、Playwright timeout、导航
evaluate 瞬态有界重试、disabled controls、storage/self ID 不一致、诊断默认关闭及
sentinel 脱敏。local fixture 已改精确 `/api/user/login` JSON 与延迟 SPA 用户状态，
并覆盖非 JSON login/challenge、关键 self 网络失败及诊断验证器拒绝不合法值。
6 秒慢响应不被抢导航，早到 self 与 optional image 302 不妨碍成功；失败入口检查
固定诊断字段和 stdout/stderr 无敏感值。实际账户、真实 SPA 初始化语义与 WAF
仍未验证；本轮未读取真实凭据或访问 AnyRouter。依赖/lock 无更改。

## 真实环境实测流程记录（2026-09-30）

后续由父代理在受限隔离临时环境中完成本次实测流程（详见 [docs/verification.md](../../docs/verification.md)）：
- 真实登录通过：通过浏览器完成本次防护页加载和登录，成功提交表单并获取会话 (`login=succeeded`)。
- 数据提取正常：成功读取原始额度（raw unit）及已有 1 个 Key (`key_count=1`)。
- 网关代理验证：本地 `/v1/models` 请求成功替换 Bearer 凭据，上游返回 HTTP 200 及 16 个模型列表 JSON。
- 运行约束：未调用付费推理模型，未创建新 Key，无自动签到。测试结束临时配置和会话文件已删除。
