# runtrol

> [!IMPORTANT]
> **最高产品规则：仅看 Runtrol 侧边栏，无需点击，就必须能查看和管理此电脑上的所有已连接编码代理 CLI、顶层项目、真实对话名称、运行状态、时间和用量。若任何信息藏在其他标签页、折叠视图、重复标签或错误层级之后，则禁止发布。**

**在一个 VS Code 窗口中即时运行所有项目、会话和代理。**

[한국어](README.md) | [English](README_EN.md) | 中文 | [日本語](README_JA.md)

> 状态：**内核和主要 VS Code 扩展已经实现，`Runtrol Studio 0.1.13` 已面向六个原生目标公开发布。** 实时会话索引、真实 Extension Host 和每秒 3,000 帧 Webview 性能门槛、已安装真实 CLI 的完整操作旅程、全新 Marketplace 安装，以及保留活动会话的 VSIX 升级与回滚都已验证。独立桌面 GUI 代码和执行路径已经删除，VS Code 扩展是唯一 PC 界面。公共 Runtime 协议、Rust 和 TypeScript SDK、外部打包消费门槛，以及六目标签名 standalone Runtime 发布流程也已经实现。已确认 provider 渠道的自动更新也具备进程互斥与精确回滚。[Marketplace 扩展](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio)、[GitHub Pages 站点](https://eddmpython.github.io/runtrol/)、基于中继的手机 PWA、无正文 Web Push 和 Mission 监督界面都已实现。对话标题栏的芯片可以在对话中切换应答模型和 permission mode，选择通过该 CLI 自身的切换界面中继，芯片只显示服务实际回答的值(已安装真实 Claude Code 的旅程门禁通过 CLI 自身的公告验证切换与恢复)。所有含有对话的文件夹都是顶层项目标题，其下只显示真实对话名称、核心状态和时间。已配对设备的权限被限定在批准的 workspace root 内：会话索引、所有会话命令和 Mission 读取都要通过同一个 live root 验证，撤销 root 立即生效。provider 准备按 provider 分车道并行，五个 cold 首次探测从串行 18.1 秒降到 8.7 秒，新打开文件夹的既有对话无需刷新即可到达。每个对话都以独立编辑器标签页打开，多个对话可以同时显示并分屏，输入与中断都指向该标签页的对话。扩展更新后仍在运行的旧内核会在机器空闲时自动滚动到新构建，更新无需重启即可真正生效。活动门禁会让已发布的 PWA 模块通过 production daemon 驱动已安装的真实 CLI，验证会话、批准和远程断线恢复旅程。在项目标题上点击一次即可启用 Agent Tools，让已安装的编码代理通过七个受项目根限制的公共 Runtime 工具委派工作；禁用时会移除 provider 注册、Runtime 权限和受保护的本地凭据。iOS 真机安装与 Web Push 运行确认仍是未经验证的贡献者 operator evidence，不计入当前完成范围和评分。
> Fleet Compare 可将同一条已审查指令一次发送到 2 至 4 个隔离 worktree 与 provider 会话，在 VS Code 网格和原生 diff 中比较结果，并只用一个选中的通过 Receipt 完成最终验证。真实双 CLI 门禁与实际 Extension Host 目视检查已经验证该流程。
> 普通 Mission 现在只需一次 `Continue Reviewed Mission` 操作，就会启动当前安全波次，用固定 Gate 封存已完成 Task，准备下一 DAG 波次，并发送精确的已审查指令。真实 CLI 与 Extension Host 的两阶段旅程已经验证该流程。
> `Continue Ready Missions` 可一次审查最多八个精确 Mission 摘要，并同时推进多个项目当前安全的波次。真实 Extension Host 用一次操作启动了两个独立 Git 项目，并在下一次操作中将两者都推进到 `integrating`。
> 下面多数分数为 0，不是因为没有代码，而是因为还没有门禁去断言那些轴。
>
> `Review and Apply Mission Landing` 会在一个 VS Code 原生多文件 diff 中，将普通 Mission 的所有通过 Receipt
> Artifact 与当前项目进行比较。一次 `Apply, run Gates and complete` 操作会重新检查 Mission、Receipt、源与
> 目标字节、链接和未保存编辑器，把已审查的精确字节应用到现有与新文件，再运行 Core 的固定 Gate。真实
> Extension Host 已在两个 Git 项目中应用四个 Artifact，拒绝项目与 Receipt 漂移并恢复，在第一个完成时保留
> 第二个等待，随后打开下一个 Landing。
>
> Fleet Compare 不再止步于比较。选择一个已通过的 Task 后，会打开只包含该 Task 与 Receipt 的原生 winner
> multi-diff。一次公开应用操作会在不混入其他候选结果的前提下写入精确字节，运行固定 Gate，并通过 Core
> 完成 Mission。Core 会把选中的 Task 与 Receipt 保留为持久终态证据，因此响应丢失后的恢复不会把其他候选
> 误判为成功。真实 Extension Host 旅程从两个不同的真实 CLI 结果中只应用了 `attempt-2`，到达
> `completed`，并直接目视检查了每个目标画面。
>
> `Mission Auto Flight` 只需在 PC 上为已审查的普通 Mission 授权一次。它通过 lifecycle generation 证明每个
> 真实 provider turn，封存固定 Gate 与 Receipt，并自动启动下一个安全 DAG 波次。遇到人员、quota 等待或暂停
> 时会保持等待，遇到权限漂移、模糊传输或恢复状态时会立即解除。真实双波次 CLI 旅程以零次操作者
> Continue 到达 `integrating`，自动收回权限，并直接目视检查了三个屏幕状态。最终 Receipt Landing 与集成
> 始终保持显式操作。
>
> 手机通知现在不会携带对话内容，并会打开第一个真正等待操作者的会话。`Needs you` 数量与下一项操作
> 只包含等待人员的状态，并与账户限额等待区分。真实 CLI 批准门禁验证等待时出现、回答后消失。
>
> Auto Flight 的人员等待、安全停止和 Receipt Landing 使用同一种无内容通知。手机通过认证后读取 Core
> 中最多 64 个结构化信号，只打开 root、Mission digest 与当前状态仍完全匹配的会话或 Mission。推送不含
> Mission ID、指令、路径或输出，手机只保留一个不透明 cursor。

The security boundary and default-deny settings are documented in [SECURITY.md](SECURITY.md).

## 北极星

**runtrol 将一个 VS Code 窗口变成所有项目、受支持的已安装编码代理 CLI 与 provider 所有会话的
control plane。每个代理都能自主修改与其绑定的仓库。runtrol 保持会话存活、隔离并发工作，且不解释
对话内容，只把所选会话连接到准确的 workspace 或 worktree。会话和代理数量可以增长，但 renderer、
active subscription 与 Code-hot workspace 始终有界。streaming 与后台工作绝不能让输入、滚动、会话切换
或文件导航卡顿。已安装的 CLI、模型和 capability 在 runtime 自动发现。对话只在用户电脑与 provider
之间往返，runtrol 不介入其中。**

### 永不改变的核心

- **功能与速度是一份合同。** 功能增加不能成为等待或卡顿的理由。可见延迟、frame drop 与输入延迟都会阻止发布。
- **多会话成本不随会话数增长。** 15 个会话是日常使用基线，30 个会话是发布门槛负载。逻辑会话可以更多，但 hot 进程最多 8 个，active renderer 与 full stream 必须始终各只有一个。固定当前选择、即时搜索、稳定排序和 workspace 切换在 30 个会话时仍使用相同操作。
- **多代理必须 provider-neutral。** 自动发现受支持的已安装 CLI，并通过统一列表和同一种操作方式运行。新增 provider 只需要 manifest 或 driver，绝不修改 core。
- **代理自主修改仓库。** provider CLI 拥有工作与对话，runtrol 只监督 session、workspace、worktree、process lifecycle 与 collision boundary。
- **对话选择与 workspace 切换绑定。** 选择 session 后立即切换对话和文件上下文，只有真正需要编辑时才把准确的 workspace 或 worktree 提升为 Code-hot。绝不读取对话内容来猜测路径。
- **设备连接与会话所有权相互分离。** VS Code 和手机只是配对到同一 Core 的操作界面，双方都不拥有会话。即使窗口、设备或网络路径改变，Core 仍保持会话存活。Tailscale 等现有私有网络可在被发现后作为直连路径，但配对、推送和正确性绝不依赖它。
- **人始终优先。** 即使存在长 streaming、多个 agent、build 与 test，输入、滚动、editor 和文件导航也必须先响应。
- **薄边界永不改变。** 不持有 provider 账户 credential、transcript、model API key 或 conversation copy。

当前总分为 **74/140，平均 5.3/10**。十三个轴由启用的 CI 门禁支撑。
10 分意味着完整旅程已在真实环境中被反复验证。
**超过 manual 层的分数必须由 CI 中真正运行的门禁支撑。不会自动执行的路径，无论看起来实现得多完整，都不能超过 3 分。**

| 北极星 | 当前分数 | 现状 | 目标状态 |
|---|---:|---|---|
| 统一的会话列表 | 5/10 | hosted CI 驱动真实 VS Code Extension Host，验证启动、两个 workspace 切换、精确选择恢复、重连、中断与关闭。对端是确定性的 loopback model，因此停留在 mock 层。 | 无论供应商是 Claude Code、Codex 还是之后出现的任何一个，此刻在我电脑上存活的会话都出现在一个列表里，启动、恢复、删除都在那里完成。 |
| 即时响应 | 5/10 | 真实 VS Code Extension Host 测量 production bundle。同一棘轮覆盖真实的 30 会话列表、最多 8 个 hot ACP 进程、provider-native cold resume、每秒 3,000 个原始帧、完成 Core watch 确认与 Webview 绘制的会话切换，以及 workspace 改变后的精确选择恢复。传输对端仍是 mock，因此该轴停留在此层级。 | 列表毫无等待地出现，对话在按下的瞬间打开，长输出倾泻而下时滚动与输入也不卡顿。用户不存在感知到加载的时刻。 |
| 用手机接续电脑上的会话 | 5/10 | hosted CI 在无界面手机进程中运行已发布 PWA 的 WebCrypto、Noise 与 CoreClient 模块，通过 production daemon 启动已安装的真实 Claude Code 会话，发送提示、监听输出并关闭会话。模型对端是确定性的 loopback fixture，因此处于 mock 层。 | 手机与电脑配对一次，之后即使离开座位，也能向那台电脑上正在运行的会话发送新指令并实时查看输出。供应商账户的等级或认证方式不会阻断这一体验。 |
| 供应商可扩展性 | 5/10 | hosted CI 检查外部驱动公开契约、三个操作系统上的通用 ACP fixture、独立发布 ACP 实现的两轮对话与 native load，以及真实 Claude Code 的隐藏审批拒绝往返。model endpoint 均为本地 mock。定时 CI 会用当前 CLI 重复 parser probe 与同一审批旅程，但不宣称覆盖账户模型行为或完整 event 表面。 | 出现新的 CLI 时只需增加一个适配器，电脑界面、手机界面与操作方式保持不变。用户只会感到列表变长了。 |
| 对话不经过 | 6/10 | `egressContract` 在真实回环套接字上运行精确的出站白名单和 production Noise IK、IKpsk1 边界。提示词样本不会以明文出现在中继捕获或诊断信息中，transport 没有磁盘或日志 API，驱动与存储也不知道供应商 transcript 路径。尚无连接真实手机与中继的 live 门禁，所以天花板是 6。 | 用户的提示词与模型的回复只在电脑与供应商之间、以及用户自己的设备之间往返。runtrol 不保存其内容，中间的任何服务器都不会以可读的形式收到它。 |
| 在手机上批准 | 5/10 | 活动门禁通过 PWA watch 路径接收真实 Claude Code 的 hidden Write 批准，验证完整 subject、唯一 `rejectOnce` 与 32-byte digest，再确认同一 provider turn 恢复并结束。模型对端是确定性的 loopback fixture，因此处于 mock 层。 | 代理在危险操作前停下时会出现在手机上，在手机上允许或拒绝后，电脑上的会话立即继续。 |
| 断了也活着 | 5/10 | 已发布的 PWA 模块和已安装的真实 CLI 在网络断开后按精确 cursor 重放，并在 Core 重启后通过明确 gap 和 native resume 继续。model 对端是 mock。 | 手机锁屏、网络中断或 runtrol 重启后，PC 会话仍可通过官方 resume surface 恢复。保留窗口内按精确 cursor 接续，窗口外明确显示 gap，绝不静默跳过。 |
| 常驻成本 | 6/10 | 三个 hosted 操作系统都用同一个棘轮测量真实 debug daemon 的 idle RSS 与十秒 idle CPU。没有第二种独立证据，因此上限仍为 6。 | 整天开着，用户也察觉不到它的存在。电池、风扇、任务管理器里都看不见。 |
| 到哪都一样 | 8/10 | 启用的 hosted CI 会把精确的原生 VSIX 安装到干净的 VS Code 中，在没有手动 Core 路径的情况下发现内置 Core，打开 Runtrol，并通过公开命令打开再关闭 `Runtrol: New chat` 编写标签页。同一门禁运行于 Windows、macOS、Linux，发布矩阵还会在 x64 与 ARM64 的六个目标上重复该旅程。这里只宣称静态契约、真实旅程与多操作系统证据。 | 在 Windows、macOS、Linux 上安装方式与操作方式相同。Windows 用户无需知道 WSL 或 tmux 是什么。 |
| 自动保持最新 | 5/10 | `vscodeUpgradeRollback` 在三个操作系统中验证 VSIX 与 Core 替换期间的会话连续性。`cliUpdateRehearsal` 通过确定性 fixture 验证已确认 provider 更新的失败、精确恢复与防振荡。托管 CI 不会修改带真实账户的 provider 安装，因此证据仍处于 mock 层。 | 应用与已安装的代理 CLI 会自行保持最新；若更新破坏了会话，在用户动手之前它已经回滚。用户不存在需要关心版本的时刻。 |
| 自动识别模型 | 6/10 | hosted `modelDetectionSmoke --require-all` 在无凭据环境中安装当前真实 CLI，检查 Codex 的 `model/list` 与包含隔离 provider-owned option cache sentinel 的 Claude partial catalogue，并拒绝在 production source 中硬编码观测 identifier。它不证明特定账户的实际可用性，因此一种 live gate 的上限为 6。 | 当前账户实际可用的模型原样出现在列表中，出现新模型时无需修改 runtrol 就会显示。 |
| 会话之间不互踩 | 5/10 | 真实 Git metadata 与 production Core admission 会把同一 worktree 的子目录视为一个 writer，并原子拒绝 opening、live、closing 状态下重叠的预约。linked worktree 与操作员明确允许的共享启动保持独立。provider 是 fixture，因此属于 mock 层。 | 哪个会话在哪个文件夹改了什么始终可以区分；第二个会话将要触碰同一文件夹时，在开始前就会收到警告；供应商提供隔离手段（worktree）时，可直接在开始界面使用。 |
| AI 互相咨询 | 3/10 | 开关通过两个真实 CLI 各自的官方命令完成接线、验证与复原，并已手动实测一次真实回合中的咨询接收（2026-08-03）。`crossConsultSmoke` 驱动真实的订阅 CLI，因此在操作者自己的机器上运行；没有 hosted CI 门禁，所以停在 manual 层。 | 一个开关让两个 CLI 通过各自的官方表面（MCP）相互注册，一个 AI 在回合中直接获取另一个 AI 的意见。接线只通过各 CLI 自己的官方命令完成（不直接写配置文件），对话内容依然不经过 runtrol，用户不需要知道 MCP 是什么。 |
| 离开的自由 | 5/10 | `uninstallLeavesNoTrace` 在 runtrol home 之外保存供应商状态并完成一个回合，删除整个 home 后由新 daemon 加载同一个原生会话并完成第二个回合。对端是 ACP fixture，因此处于 mock 层。 | 删除 runtrol 后，会话与记录仍属于各个 CLI，按原来的方式继续。不存在被 runtrol 扣作人质的数据。 |

哪个门禁支撑哪个轴，以 [docs/northStarEvidence.md](docs/northStarEvidence.md) 为准。

### 评分标准

分数不是人挑选的等级，而是按**基础层加上加分**算出来的。正本是
[tests/audit/northStar/board.toml](tests/audit/northStar/board.toml)，由 `northStarBoard` 门禁计算，
再由 `readmeParity` 门禁把四种语言的 README 与计算结果对齐。

**基础层。** 每个轴只成立一个，它就是天花板。

| 基础层 | 分数 | 成立条件 |
|---|---:|---|
| `none` | 0 | 没有门禁断言这个轴 |
| `manual` | 3 | 有人手动看它跑通过一次。hosted CI 中没有启用的门禁。演示视频、截图、"我跑过了能用" 全在这里 |
| `mock` | 5 | 已登记的门禁在运行，但对手是假的。mock CLI、stub 供应商、模拟的手机 |
| `realOneKind` | 6 | 对真实对手运行，但 static (`contract`) 与 live (`smoke`、`bench`) 只有一种 |
| `realBothKinds` | 7 | 对真实对手运行，且 static 与 live 两种都具备 |

**加分。** 只在 `realBothKinds` 上附加。每一项都要求相应种类的门禁，四项齐备时正好是 10。

| 加分 | 分数 | 成立条件 |
|---|---:|---|
| `multiProvider` | +1 | 同一门禁在两个以上供应商上为 green |
| `multiOs` | +1 | 同一门禁在两个以上操作系统上为 green，含 Windows |
| `faultInjection` | +0.5 | 门禁带着故障注入（强杀守护进程、切断网络）仍为 green |
| `ratchet` | +0.5 | 有回归 ratchet，数字一变差立刻变红 |

防止分数虚高的规则：

1. **无论实现看起来多完整，只要没有运行器真正调用的门禁，上限就是 `manual`（3 分）。** 没有例外。
2. `operator` 类门禁（需要真实设备或真实账号的那些）**从总分中扣除**。
3. 提升分数的 PR 必须在正文附上**门禁名称与 CI 运行链接**，并在同一次提交里改 `board.toml`。散文不是分数。
4. 分数只以 0.5 为单位。8.7 这样的数字不是精确，而是自欺。
5. **不阻止某个轴下降。** 供应商改变表面导致门禁变红时，分数就该下降。这张表是今天的状态，不是昨天的炫耀。
6. **决定天花板的是缺失的门禁种类，不是运行次数。** 只有一种门禁的轴，无论跑得多绿都过不了 6。目前 14 个轴里有 13 个处在这个状态，`northStarBoard` 会在每个轴旁边印出它的天花板。

### 什么计分，什么不计分

三层互不混合。一混合，用户什么也没拿到，总分却涨了。

| 层 | 里面放什么 | 如何呈现 |
|---|---|---|
| **计分轴** | 用户能感受到的结果（上表的 14 个） | 0 到 10，合计 /140 |
| **底线门禁** | 模块化、整洁代码、安全、卫生、预算 | **不是分数。** 只有 green 或 red，red 不予合并 |
| **撤退条件** | 创新性、定位 | **没有数字。** 只由 [docs/positioning.md](docs/positioning.md) 的 kill criteria 判定 |

- **为什么模块化与整洁代码不给部分分。** 两者都是强制规则。"整洁代码 7/10" 的意思是"正在违规 3 分"，那不是分数，那是 red。它们变细的方式是拆成一个个具名门禁（`dependencyDirection`、`providerIsolation`、`checkSilentFail`、`cargoClippy` 等），完整清单在 [docs/northStarEvidence.md](docs/northStarEvidence.md)。
- **为什么创新性不给数字。** 创新就是上面这 14 个轴本身（"在一个地方管理多个 AI"）。单独给分等于把同一件事数两遍，而且没有任何门禁能断言那个数字，正好撞上规则 3。创新是否还成立，由 kill criteria 判定。

## 最高原则：用户便利

每一个岔路口都选对用户更方便的一侧。判定标准不是品味，而是**用户实际执行的操作数量与等待的时间**。

- 本该自动完成却需要用户配置，就是失败
- 用户能看见自己在等待，就是失败
- 用户必须学习某个概念（tmux、WSL、隧道、端口转发、安装证书），就是失败
- 用户做同一件事两次，就是失败
- **卡顿是缺陷，不是优化项**

## 获取

| | |
|---|---|
| **PC（Windows、macOS、Linux）** | 从 [VS Code Marketplace 安装 `Runtrol Studio`](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio)。支持 x64 与 ARM64，不会分发独立桌面应用 |
| **移动端** | [永久 GitHub Pages 地址上的手机 PWA](https://eddmpython.github.io/runtrol/app/)。先使用 VS Code 中的一次性二维码配对 |

公开版本 `0.1.13` 与六个平台 VSIX 也可从 [GitHub Releases](https://github.com/eddmpython/runtrol/releases/tag/vscode-v0.1.13) 获取。
VS Code 会自动更新从 Marketplace 安装的扩展。如果旧版本是直接通过 VSIX 安装的，请从 Marketplace 重新安装一次，因为 VS Code 会关闭手动 VSIX 安装的自动更新。

## 让代理使用 Runtrol

点击项目标题上的闪光按钮并选择 **Enable Agent Tools for This Project**。已安装的编码代理可以发现
provider 和模型、启动项目会话、发送指令、读取事件并停止准确的会话。项目行显示 `Agent Tools` 时即已就绪。

权限只绑定到该 canonical 项目根。它不能回答批准、删除对话、静默共享工作树，也不持有 API key、
transcript 副本或 Runtrol 自有 agent loop。选择 **Disable Agent Tools for This Project** 会移除 Runtime
权限与受 OS 保护的凭据；禁用最后一个项目时也会移除 provider 注册。完整契约见
[Agent Tools](docs/agentTools.md)。

## 不需要 runtrol 的人

**如果你只用一个供应商，那么该供应商自己的远程控制更好。这一点先写在前面。**

只用 Claude Code 的人，`claude --remote-control` 更好。做它的人做的，免费捆绑，
带原生推送，还在应用商店里。
Anthropic、OpenAI、GitHub、Amp 四家都已推出各自的远程控制。够用就用它。

**runtrol 是给那些列表被拆成四份的人的。**
Codex 会话永远不会出现在 Claude 应用里。这不是功能差距，而是结构性的，供应商没有理由去修。

## runtrol 不是什么

- **不是聊天客户端。** 渲染对话是各个 CLI 已经在做的事。runtrol 只搬运输出，不解释它。
- **不是模型代理。** 不调用模型 API，不读取令牌，不中继请求。这不是设计偏好，而是生存条件。
- **不是 IDE。** 展示 diff 是边界，编辑 diff 在边界之外。
- **不是自有代理框架。** Runtrol 不拥有规划器或自主循环。它向 provider 自有 agent loop 提供受限 Runtime 工具，但不会成为那个 loop。
- **不是托管服务。** 没有账户，没有登录，没有套餐。
- **不是终端复用器。** 目标不是取代 tmux，而是**不要求 tmux**。

## 为什么用 Rust

诚实地说，**Rust 本身不是差异点。** 这个领域已有十几个竞品在用 Rust。
Rust 不是目的，而是上表中三个轴的手段。

- **到哪都一样**：在同一抽象后直接处理 ConPTY 与 POSIX，让 Windows 在没有 tmux 的情况下成为一等公民。
- **常驻成本**：这是整天开着的守护进程。没有运行时的单一静态二进制意味着无需安装 Node 或 Python。
- **即时响应**：列表与对话毫无等待地打开，需要没有 GC 停顿与运行时启动。

如果不用门禁把这些轴钉死，用 Rust 就失去了意义。

## 目录结构

| | | |
|---|---|---|
| `crates/` | 产品内核（Rust）。守护进程、供应商适配器与传输。不存在独立 GUI crate | 已实现 |
| [`clients/typescript/`](clients/typescript/) | 面向外部产品的公共 Runtime TypeScript SDK | 已验证打包消费 |
| [`extensions/runtrol-vscode/`](extensions/runtrol-vscode/) | 唯一 PC 界面 `Runtrol Studio` | 30 会话发布负载已验证，0.1.13 已公开 |
| [`pwa/`](pwa/) | 移动端 PWA | 已实现中继连接、会话控制、批准以及 `Needs you` 与 Mission Flight Signals 精确直达 |
| [`site/`](site/) | [无依赖 GitHub Pages 落地页](https://eddmpython.github.io/runtrol/) | 已上线 |
| [`assets/brand/`](assets/brand/) | 标志。SVG 为正本，favicon、图标与社交卡片皆由其派生 | |
| [`docs/`](docs/README.md) | 运营文档正本 | |
| [`tests/audit/`](tests/audit/) | 契约门禁 | |
| [`tests/audit/northStar/`](tests/audit/northStar/) | 计分板引擎。计算上表的数字并对齐四种语言 | |

## 开发

```bash
python -X utf8 tests/audit/preflight.py          # 本地完整 CI
python -X utf8 tests/audit/preflight.py lint     # 仅 lint
python -X utf8 tests/audit/preflight.py --list   # 哪些会跑，哪些被跳过
git config core.hooksPath .githooks              # 每次克隆执行一次
```

门禁是**缺陷探测器，而不是通过图章**。新建门禁时，在看到它通过之前，
先故意植入它应当捕获的缺陷，确认它会变红
（`python -X utf8 tests/audit/checkSilentFail.py --selftest` 就是这个形态）。

参与贡献请看 [CONTRIBUTING.md](CONTRIBUTING.md)。设计阶段的贡献也是真正的贡献。

## 许可证

产品本体采用 [AGPL-3.0-only](LICENSE)。公开客户端包 (`runtrol-runtime-protocol` ·
`runtrol-runtime-client` · `@runtrol/runtime-client`) 是供其他程序链接的，因此采用
[Apache-2.0](crates/runtrol-runtime-protocol/LICENSE)。

仅仅使用 runtrol 不会给你的代码带来任何义务。runtrol 只是把智能体 CLI 作为独立进程来监督，
不会链接进你写的任何东西。
