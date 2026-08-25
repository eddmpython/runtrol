import { mountIcons } from "./icons.js";
import { inferNativeTarget, selectTargetVsix } from "./release-assets.mjs";
import { startScene } from "./scene.js";

const PROJECT = Object.freeze({
  repository: "https://github.com/eddmpython/runtrol",
  releasesApi: "https://api.github.com/repos/eddmpython/runtrol/releases/latest",
});

const COPY = {
  en: {
    navSidebar: "Sidebar",
    navInstall: "Install",
    navPhone: "Phone",
    navThin: "Principles",
    heroEyebrow: "Open source. Thin by design. Inside VS Code.",
    heroTitle: "Every coding agent.<br>One VS Code sidebar.",
    heroLede: "runtrol supervises the coding-agent CLIs already installed on your machine. Every project, every conversation, who is running, and how much quota is left, in one sidebar, with no extra window and no second copy of your chats.",
    installMarketplace: "Install from Marketplace",
    viewSource: "View source",
    releaseChecking: "Checking GitHub Releases for a VSIX.",
    releaseMissing: "No public VSIX has been released yet.",
    releaseFound: "A manual VSIX is available from GitHub Releases. Marketplace installation is required for automatic updates.",
    currentFolder: "current folder",
    usage: "Usage",
    turnDone: "Turn finished",
    composer: "Message",
    approvalAsk: "Allow: write Cargo.toml",
    allow: "Allow",
    needsYou: "Needs you: 1",
    needsYouBody: "Migrate config loader is waiting for approval",
    proofWindow: "VS Code window",
    proofSessions: "conversations under one release gate",
    proofCopies: "transcript copies",
    proofKeys: "model API keys held",
    featuresTitle: "Everything in the sidebar. Nothing hidden behind a tab.",
    featuresIntro: "The release rule is strict: every connected CLI, every top-level project, the real conversation title, who is running, and how much quota is left must be readable from one screen without a click.",
    f1Title: "One tree, every project",
    f1Body: "The current folder is the first heading. Folders you keep returning to become projects on their own.",
    f2Title: "Icons that spin while work runs",
    f2Body: "Each row shows the agent and the real conversation title. The icon itself rotates while that turn is running.",
    f3Title: "Quota you can see",
    f3Body: "Per-CLI usage and reset time stay in view, read from the provider itself, never estimated.",
    f4Title: "Chats as editor tabs",
    f4Body: "Each conversation opens as its own tab. Split them like files. Typing and interrupting go to that tab.",
    f5Title: "Ask several CLIs at once",
    f5Body: "Send the same first message to Claude Code and Codex in separate linked worktrees, then compare in native diff.",
    f6Title: "Updates without a restart",
    f6Body: "After an extension update the old Core rolls to the new build the moment the machine is idle.",
    installTitle: "Install once. runtrol finds the CLIs.",
    installIntro: "There is no separate desktop app. Runtrol Studio lives where the work already happens and discovers installed CLIs, their versions, models, and flags at runtime.",
    stepOneTitle: "Install Runtrol Studio",
    stepOneBody: "From the Visual Studio Marketplace, or search <code>@id:runtrol.runtrol-studio</code> in VS Code.",
    stepTwoTitle: "Open the runtrol icon",
    stepTwoBody: "The sidebar opens inside your current VS Code window with the current folder already listed.",
    stepThreeTitle: "Keep your CLIs as they are",
    stepThreeBody: "Each CLI stays installed and signed in through its own official flow. runtrol never touches its credentials.",
    stepFourTitle: "Pick a conversation and go",
    stepFourBody: "Cold sessions resume through the provider's own identity. The workspace follows what you selected.",
    primaryInstall: "PRIMARY INSTALL",
    primaryInstallState: "Public for six native targets, updated through VS Code.",
    openMarketplace: "Open Marketplace",
    manualInstall: "MANUAL FALLBACK",
    vsixUnavailable: "No public VSIX is available yet.",
    checkReleases: "Check Releases",
    downloadVsix: "Download VSIX",
    chooseVsix: "Choose platform",
    releaseChoose: "Choose the VSIX that matches your operating system and architecture.",
    phoneTitle: "Pair once. The PC keeps owning the session.",
    phoneBody: "The phone app is live at this permanent HTTPS origin. It pairs from a one-use QR in VS Code and reconnects through an end-to-end encrypted relay. Notifications carry no conversation content and open the exact session that needs you.",
    phoneProgress: "Secure phone app available",
    phoneOpen: "Open phone app",
    principlesTitle: "The supervisor stays out of the conversation.",
    p1Title: "Provider-owned history",
    p1Body: "runtrol transports process events. It never keeps a second transcript or rewrites meaning.",
    p2Title: "Runtime discovery",
    p2Body: "Providers, versions, models, flags, and session paths come from the installed CLI, not from a hardcoded list.",
    p3Title: "Remote default deny",
    p3Body: "Phone control is scoped, paired, and encrypted. Model credentials never cross runtrol.",
    footerText: "Open source. Built around the sessions and tools you already own.",
  },
  ko: {
    navSidebar: "사이드바",
    navInstall: "설치",
    navPhone: "휴대폰",
    navThin: "원칙",
    heroEyebrow: "오픈소스. 얇은 설계. VS Code 안에서.",
    heroTitle: "모든 코딩 에이전트를<br>VS Code 사이드바 하나로.",
    heroLede: "runtrol은 컴퓨터에 이미 설치된 코딩 에이전트 CLI를 감독합니다. 모든 프로젝트, 모든 대화, 지금 누가 돌고 있는지, 사용량이 얼마나 남았는지를 사이드바 한 화면에서 봅니다. 별도 창도, 대화 사본도 없습니다.",
    installMarketplace: "Marketplace에서 설치",
    viewSource: "소스 보기",
    releaseChecking: "GitHub Releases에서 VSIX를 확인하고 있습니다.",
    releaseMissing: "아직 공개 VSIX가 없습니다.",
    releaseFound: "GitHub Releases에서 수동 설치용 VSIX를 받을 수 있습니다. 자동 갱신은 Marketplace 설치를 사용합니다.",
    currentFolder: "현재 폴더",
    usage: "사용량",
    turnDone: "턴 완료",
    composer: "메시지",
    approvalAsk: "허용: Cargo.toml 쓰기",
    allow: "허용",
    needsYou: "확인 필요: 1",
    needsYouBody: "Migrate config loader 가 승인을 기다립니다",
    proofWindow: "VS Code 창",
    proofSessions: "대화가 한 릴리즈 게이트 아래",
    proofCopies: "대화 사본",
    proofKeys: "보유한 모델 API 키",
    featuresTitle: "전부 사이드바에. 탭 뒤에 숨기지 않습니다.",
    featuresIntro: "릴리즈 규칙은 엄격합니다. 연결된 모든 CLI, 모든 최상위 프로젝트, 실제 대화명, 누가 돌고 있는지, 사용량이 얼마나 남았는지를 클릭 없이 한 화면에서 읽을 수 있어야 합니다.",
    f1Title: "트리 하나에 모든 프로젝트",
    f1Body: "현재 폴더가 첫 헤딩입니다. 자주 여는 폴더는 저절로 프로젝트가 됩니다.",
    f2Title: "돌고 있으면 아이콘이 돕니다",
    f2Body: "각 행은 에이전트와 실제 대화명만 보여 줍니다. 턴이 도는 동안 아이콘 자체가 회전합니다.",
    f3Title: "눈에 보이는 사용량",
    f3Body: "CLI별 사용량과 초기화 시각이 늘 보입니다. provider가 답한 값이지 추정이 아닙니다.",
    f4Title: "대화는 에디터 탭",
    f4Body: "대화마다 탭으로 열립니다. 파일처럼 분할하고, 입력과 중단은 그 탭의 대화로 갑니다.",
    f5Title: "여러 CLI에 한 번에 묻기",
    f5Body: "같은 첫 메시지를 Claude Code와 Codex에 각자의 worktree로 보내고 native diff로 비교합니다.",
    f6Title: "재시작 없는 업데이트",
    f6Body: "확장이 갱신되면 옛 Core는 기계가 유휴가 되는 즉시 새 빌드로 굴려집니다.",
    installTitle: "한 번 설치하면 runtrol이 CLI를 찾습니다.",
    installIntro: "별도 데스크톱 앱이 없습니다. Runtrol Studio는 일이 벌어지는 곳에 살고, 설치된 CLI와 그 버전·모델·플래그를 런타임에 발견합니다.",
    stepOneTitle: "Runtrol Studio 설치",
    stepOneBody: "Visual Studio Marketplace에서, 또는 VS Code에서 <code>@id:runtrol.runtrol-studio</code> 를 검색합니다.",
    stepTwoTitle: "runtrol 아이콘 열기",
    stepTwoBody: "현재 VS Code 창 안에 사이드바가 열리고 현재 폴더가 이미 올라와 있습니다.",
    stepThreeTitle: "CLI는 그대로 둡니다",
    stepThreeBody: "각 CLI는 자기 공식 절차로 설치되고 로그인된 채 남습니다. runtrol은 그 자격증명을 만지지 않습니다.",
    stepFourTitle: "대화를 고르고 바로 시작",
    stepFourBody: "식은 세션은 provider 자신의 식별자로 재개됩니다. 워크스페이스는 선택을 따라갑니다.",
    primaryInstall: "기본 설치",
    primaryInstallState: "6개 네이티브 대상에 공개, VS Code가 갱신합니다.",
    openMarketplace: "Marketplace 열기",
    manualInstall: "수동 대안",
    vsixUnavailable: "아직 공개 VSIX가 없습니다.",
    checkReleases: "Releases 확인",
    downloadVsix: "VSIX 다운로드",
    chooseVsix: "플랫폼 선택",
    releaseChoose: "운영체제와 아키텍처에 맞는 VSIX를 고릅니다.",
    phoneTitle: "한 번 페어링하면 세션의 주인은 PC입니다.",
    phoneBody: "휴대폰 앱은 이 영구 HTTPS 주소에 있습니다. VS Code의 일회용 QR로 페어링하고 종단간 암호화된 릴레이로 다시 연결합니다. 알림은 대화 내용을 싣지 않고, 당신을 기다리는 바로 그 세션을 엽니다.",
    phoneProgress: "보안 휴대폰 앱 사용 가능",
    phoneOpen: "휴대폰 앱 열기",
    principlesTitle: "감독자는 대화 밖에 머뭅니다.",
    p1Title: "provider가 소유한 이력",
    p1Body: "runtrol은 프로세스 이벤트를 전송할 뿐입니다. 두 번째 transcript를 두거나 의미를 고쳐 쓰지 않습니다.",
    p2Title: "런타임 발견",
    p2Body: "provider, 버전, 모델, 플래그, 세션 경로는 하드코딩 목록이 아니라 설치된 CLI에서 옵니다.",
    p3Title: "원격은 기본 거부",
    p3Body: "휴대폰 제어는 범위가 제한되고 페어링되고 암호화됩니다. 모델 자격증명은 runtrol을 지나지 않습니다.",
    footerText: "오픈소스. 이미 가진 세션과 도구를 중심으로 만들었습니다.",
  },
  zh: {
    navSidebar: "侧边栏",
    navInstall: "安装",
    navPhone: "手机",
    navThin: "原则",
    heroEyebrow: "开源。轻薄设计。就在 VS Code 里。",
    heroTitle: "所有编码代理，<br>一个 VS Code 侧边栏。",
    heroLede: "runtrol 监督你电脑上已经安装的编码代理 CLI。每个项目、每段对话、谁在运行、配额还剩多少，都在一个侧边栏里，没有额外窗口，也没有对话副本。",
    installMarketplace: "从 Marketplace 安装",
    viewSource: "查看源码",
    releaseChecking: "正在检查 GitHub Releases 中的 VSIX。",
    releaseMissing: "尚未发布公开 VSIX。",
    releaseFound: "GitHub Releases 提供手动安装的 VSIX。自动更新需要 Marketplace 安装。",
    currentFolder: "当前文件夹",
    usage: "用量",
    turnDone: "本轮完成",
    composer: "消息",
    approvalAsk: "允许：写入 Cargo.toml",
    allow: "允许",
    needsYou: "需要你：1",
    needsYouBody: "Migrate config loader 正在等待批准",
    proofWindow: "个 VS Code 窗口",
    proofSessions: "段对话共用一个发布门槛",
    proofCopies: "份对话副本",
    proofKeys: "个模型 API 密钥",
    featuresTitle: "一切都在侧边栏。没有东西藏在标签页后面。",
    featuresIntro: "发布规则很严格：每个已连接的 CLI、每个顶层项目、真实对话标题、谁在运行、配额还剩多少，都必须在一个画面里无需点击即可读到。",
    f1Title: "一棵树，所有项目",
    f1Body: "当前文件夹是第一个标题。你反复打开的文件夹会自动成为项目。",
    f2Title: "运行时图标会旋转",
    f2Body: "每一行只显示代理和真实对话标题。本轮运行期间图标本身会旋转。",
    f3Title: "看得见的配额",
    f3Body: "各 CLI 的用量和重置时间始终可见，来自 provider 本身，不是估算。",
    f4Title: "对话即编辑器标签页",
    f4Body: "每段对话在自己的标签页打开。像文件一样分屏，输入和中断都指向该标签页。",
    f5Title: "同时问多个 CLI",
    f5Body: "把同一条首条消息发给 Claude Code 和 Codex 各自的 worktree，再用原生 diff 比较。",
    f6Title: "无需重启的更新",
    f6Body: "扩展更新后，旧 Core 会在机器空闲时立即滚动到新构建。",
    installTitle: "安装一次，runtrol 自己找到 CLI。",
    installIntro: "没有独立的桌面应用。Runtrol Studio 就住在工作发生的地方，在运行时发现已安装的 CLI 及其版本、模型和参数。",
    stepOneTitle: "安装 Runtrol Studio",
    stepOneBody: "从 Visual Studio Marketplace 安装，或在 VS Code 中搜索 <code>@id:runtrol.runtrol-studio</code>。",
    stepTwoTitle: "打开 runtrol 图标",
    stepTwoBody: "侧边栏在当前 VS Code 窗口中打开，当前文件夹已经列在上面。",
    stepThreeTitle: "CLI 保持原样",
    stepThreeBody: "每个 CLI 按自己的官方流程安装和登录。runtrol 不会触碰它们的凭据。",
    stepFourTitle: "选一段对话，开始",
    stepFourBody: "冷会话通过 provider 自己的标识恢复。工作区跟随你的选择。",
    primaryInstall: "主要安装方式",
    primaryInstallState: "面向六个原生目标公开，通过 VS Code 更新。",
    openMarketplace: "打开 Marketplace",
    manualInstall: "手动备选",
    vsixUnavailable: "尚无公开 VSIX。",
    checkReleases: "查看 Releases",
    downloadVsix: "下载 VSIX",
    chooseVsix: "选择平台",
    releaseChoose: "选择与你的操作系统和架构匹配的 VSIX。",
    phoneTitle: "配对一次，会话仍归 PC 所有。",
    phoneBody: "手机应用位于这个永久 HTTPS 地址。它通过 VS Code 中的一次性二维码配对，并经端到端加密中继重连。通知不携带对话内容，直接打开需要你的那段会话。",
    phoneProgress: "安全手机应用已可用",
    phoneOpen: "打开手机应用",
    principlesTitle: "监督者不介入对话。",
    p1Title: "provider 持有历史",
    p1Body: "runtrol 只传输进程事件。它从不保留第二份 transcript，也不改写含义。",
    p2Title: "运行时发现",
    p2Body: "provider、版本、模型、参数和会话路径来自已安装的 CLI，而不是硬编码列表。",
    p3Title: "远程默认拒绝",
    p3Body: "手机控制有范围限制、需配对且加密。模型凭据永远不经过 runtrol。",
    footerText: "开源。围绕你已经拥有的会话和工具构建。",
  },
  ja: {
    navSidebar: "サイドバー",
    navInstall: "インストール",
    navPhone: "スマートフォン",
    navThin: "原則",
    heroEyebrow: "オープンソース。薄い設計。VS Code の中で。",
    heroTitle: "すべてのコーディングエージェントを<br>VS Code のサイドバー一つに。",
    heroLede: "runtrol はこの PC にすでに入っているコーディングエージェント CLI を監督します。すべてのプロジェクト、すべての会話、いま誰が動いているか、残り使用量を一つのサイドバーで。別ウィンドウも会話のコピーもありません。",
    installMarketplace: "Marketplace からインストール",
    viewSource: "ソースを見る",
    releaseChecking: "GitHub Releases の VSIX を確認しています。",
    releaseMissing: "公開 VSIX はまだありません。",
    releaseFound: "GitHub Releases から手動インストール用 VSIX を取得できます。自動更新には Marketplace インストールが必要です。",
    currentFolder: "現在のフォルダー",
    usage: "使用量",
    turnDone: "ターン完了",
    composer: "メッセージ",
    approvalAsk: "許可: Cargo.toml への書き込み",
    allow: "許可",
    needsYou: "要対応: 1",
    needsYouBody: "Migrate config loader が承認を待っています",
    proofWindow: "つの VS Code ウィンドウ",
    proofSessions: "会話が一つのリリースゲートの下に",
    proofCopies: "件の会話コピー",
    proofKeys: "件のモデル API キー",
    featuresTitle: "すべてサイドバーに。タブの裏に隠さない。",
    featuresIntro: "リリース規則は厳格です。接続されたすべての CLI、すべてのトップレベルプロジェクト、実際の会話名、誰が動いているか、残り使用量を、クリックなしに一画面で読めなければなりません。",
    f1Title: "一つのツリーにすべてのプロジェクト",
    f1Body: "現在のフォルダーが最初の見出しです。繰り返し開くフォルダーは自然にプロジェクトになります。",
    f2Title: "動いている間はアイコンが回る",
    f2Body: "各行はエージェントと実際の会話名だけを示します。ターンの実行中はアイコン自体が回転します。",
    f3Title: "見える使用量",
    f3Body: "CLI ごとの使用量とリセット時刻が常に見えます。provider 自身の値で、推定ではありません。",
    f4Title: "会話はエディタータブ",
    f4Body: "会話ごとにタブで開きます。ファイルのように分割し、入力と中断はそのタブの会話に向かいます。",
    f5Title: "複数の CLI に同時に聞く",
    f5Body: "同じ最初のメッセージを Claude Code と Codex にそれぞれの worktree で送り、native diff で比較します。",
    f6Title: "再起動なしの更新",
    f6Body: "拡張の更新後、旧 Core はマシンがアイドルになった瞬間に新しいビルドへ入れ替わります。",
    installTitle: "一度入れれば runtrol が CLI を見つけます。",
    installIntro: "独立したデスクトップアプリはありません。Runtrol Studio は作業が起きる場所に住み、インストール済み CLI とそのバージョン、モデル、フラグを実行時に発見します。",
    stepOneTitle: "Runtrol Studio をインストール",
    stepOneBody: "Visual Studio Marketplace から、または VS Code で <code>@id:runtrol.runtrol-studio</code> を検索します。",
    stepTwoTitle: "runtrol アイコンを開く",
    stepTwoBody: "現在の VS Code ウィンドウ内にサイドバーが開き、現在のフォルダーがすでに並んでいます。",
    stepThreeTitle: "CLI はそのままに",
    stepThreeBody: "各 CLI は自身の公式手順でインストールとログインされたままです。runtrol はその資格情報に触れません。",
    stepFourTitle: "会話を選んで進む",
    stepFourBody: "冷えたセッションは provider 自身の識別子で再開されます。ワークスペースは選択に従います。",
    primaryInstall: "主なインストール",
    primaryInstallState: "六つの native target に公開、VS Code が更新します。",
    openMarketplace: "Marketplace を開く",
    manualInstall: "手動の代替",
    vsixUnavailable: "公開 VSIX はまだありません。",
    checkReleases: "Releases を確認",
    downloadVsix: "VSIX をダウンロード",
    chooseVsix: "プラットフォームを選択",
    releaseChoose: "OS とアーキテクチャに合う VSIX を選びます。",
    phoneTitle: "一度ペアリングすれば、セッションの所有者は PC のままです。",
    phoneBody: "スマートフォンアプリはこの恒久的な HTTPS オリジンにあります。VS Code の一回限りの QR でペアリングし、エンドツーエンド暗号化されたリレーで再接続します。通知は会話内容を含まず、あなたを待つそのセッションを開きます。",
    phoneProgress: "安全なスマートフォンアプリが利用可能",
    phoneOpen: "スマートフォンアプリを開く",
    principlesTitle: "監督者は会話の外にとどまります。",
    p1Title: "provider が持つ履歴",
    p1Body: "runtrol はプロセスイベントを運ぶだけです。二つ目の transcript を持たず、意味を書き換えません。",
    p2Title: "実行時の発見",
    p2Body: "provider、バージョン、モデル、フラグ、セッションパスはハードコードされた一覧ではなく、インストール済み CLI から来ます。",
    p3Title: "リモートは既定で拒否",
    p3Body: "スマートフォン制御は範囲が限られ、ペアリングされ、暗号化されます。モデルの資格情報は runtrol を通りません。",
    footerText: "オープンソース。すでに持っているセッションとツールを中心に作られています。",
  },
};

const THEME_KEY = "runtrol.landing.theme";
const LANGUAGE_KEY = "runtrol.landing.language";

function readPreference(key) {
  try {
    return window.localStorage.getItem(key);
  } catch {
    // Storage can be blocked by the browser; the page must still render with defaults.
    return null;
  }
}

function writePreference(key, value) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Same as above: a blocked store only loses the preference, not the page.
  }
}

function detectLanguage() {
  const stored = readPreference(LANGUAGE_KEY);
  if (stored && COPY[stored]) {
    return stored;
  }
  const preferred = (navigator.language ?? "en").slice(0, 2).toLowerCase();
  return COPY[preferred] ? preferred : "en";
}

function applyLanguage(language) {
  const dictionary = COPY[language] ?? COPY.en;
  document.documentElement.lang = language;
  for (const node of document.querySelectorAll("[data-i18n]")) {
    const key = node.dataset.i18n;
    const text = dictionary[key] ?? COPY.en[key];
    if (text !== undefined) {
      node.innerHTML = text;
    }
  }
  const select = document.getElementById("language");
  if (select) {
    select.value = language;
  }
  return dictionary;
}

function detectTheme() {
  const stored = readPreference(THEME_KEY);
  if (stored === "light" || stored === "dark") {
    return stored;
  }
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
}

async function discoverRelease(dictionary) {
  const status = document.getElementById("release-status");
  const button = document.getElementById("vsix-download");
  const description = document.getElementById("vsix-description");
  try {
    const response = await fetch(PROJECT.releasesApi, { headers: { Accept: "application/vnd.github+json" } });
    if (!response.ok) {
      throw new Error(`GitHub Releases responded ${response.status}`);
    }
    const release = await response.json();
    const assets = Array.isArray(release.assets) ? release.assets : [];
    const vsix = assets.filter((asset) => typeof asset.name === "string" && asset.name.endsWith(".vsix"));
    if (vsix.length === 0) {
      status.textContent = dictionary.releaseMissing;
      return;
    }
    const target = inferNativeTarget(navigator.userAgentData ?? { platform: navigator.platform });
    const matched = selectTargetVsix(assets, target);
    status.textContent = dictionary.releaseFound;
    button.classList.remove("is-disabled");
    button.removeAttribute("aria-disabled");
    if (matched) {
      button.href = matched.browser_download_url;
      button.textContent = dictionary.downloadVsix;
      description.textContent = `${matched.name} (${release.tag_name})`;
    } else {
      button.href = release.html_url;
      button.textContent = dictionary.chooseVsix;
      description.textContent = dictionary.releaseChoose;
    }
  } catch (error) {
    // A network failure must not hide the Marketplace path; the static copy already says a VSIX is not confirmed.
    status.textContent = dictionary.releaseMissing;
    console.warn("release discovery failed", error);
  }
}

function main() {
  mountIcons(document);

  applyTheme(detectTheme());
  document.getElementById("theme-toggle")?.addEventListener("click", () => {
    const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    applyTheme(next);
    writePreference(THEME_KEY, next);
  });

  let dictionary = applyLanguage(detectLanguage());
  document.getElementById("language")?.addEventListener("change", (event) => {
    const language = event.target.value;
    dictionary = applyLanguage(language);
    writePreference(LANGUAGE_KEY, language);
  });

  const studio = document.getElementById("sidebar");
  if (studio) {
    startScene(studio);
  }

  void discoverRelease(dictionary);
}

main();
